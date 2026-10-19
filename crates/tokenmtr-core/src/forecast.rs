//! Forecasts: quota exhaustion ETA, period spend projection and live burn rate.

use std::collections::{BTreeMap, BTreeSet};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::event::UsageEvent;
use crate::ids::{AgentId, SessionId};
use crate::money::{Money, MoneyError, UsdMicros, saturating_micros};
use crate::quota::{QuotaSnapshot, WindowId};

/// Minimum snapshots a quota forecast needs.
pub const MIN_ETA_SAMPLES: usize = 3;
/// Minimum time the snapshots must span for a quota forecast.
pub const MIN_ETA_SPAN: SignedDuration = SignedDuration::from_mins(15);
/// How far two reported `resets_at` values may differ and still be the same window instance.
pub const RESET_TOLERANCE: SignedDuration = SignedDuration::from_mins(5);
/// Days of history averaged by [`project_period`].
pub const PROJECTION_DAYS: usize = 7;

/// When a quota window is forecast to run out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum QuotaEta {
    /// The trend reaches 100% before the window resets.
    WillHit {
        /// Forecast moment of exhaustion (never earlier than the last snapshot).
        at: Timestamp,
    },
    /// The trend stays under 100% until the reset.
    WontHit,
    /// Fewer than [`MIN_ETA_SAMPLES`] snapshots, or they span less than [`MIN_ETA_SPAN`].
    NotEnoughData,
}

/// Converts a small count to `f64` exactly.
fn count_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Least-squares forecast of when `window` reaches 100%.
///
/// Only snapshots of the window instance that resets at `resets_at` (within five minutes, as
/// providers jitter the value) and taken no later than the reset are used, so a window reset
/// never contaminates the fit.
#[must_use]
pub fn quota_eta(snapshots: &[QuotaSnapshot], window: &WindowId, resets_at: Timestamp) -> QuotaEta {
    let mut samples: Vec<(Timestamp, f64)> = snapshots
        .iter()
        .filter(|s| s.fetched_at <= resets_at)
        .filter_map(|s| {
            let state = s.window(window)?;
            let reset = state.resets_at?;
            (reset.duration_since(resets_at).abs() <= RESET_TOLERANCE)
                .then_some((s.fetched_at, state.utilization.percent()))
        })
        .collect();
    samples.sort_by_key(|(at, _)| *at);
    let (Some(&(first_at, _)), Some(&(last_at, last_percent))) = (samples.first(), samples.last())
    else {
        return QuotaEta::NotEnoughData;
    };
    if samples.len() < MIN_ETA_SAMPLES || last_at.duration_since(first_at) < MIN_ETA_SPAN {
        return QuotaEta::NotEnoughData;
    }
    if last_percent >= 100.0 {
        return QuotaEta::WillHit { at: last_at };
    }

    let points: Vec<(f64, f64)> = samples
        .iter()
        .map(|&(at, y)| (at.duration_since(first_at).as_secs_f64(), y))
        .collect();
    let n = count_f64(points.len());
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    if sxx <= 0.0 || sxy <= 0.0 {
        return QuotaEta::WontHit;
    }
    let slope = sxy / sxx;
    let hit_secs = mean_x + (100.0 - mean_y) / slope;
    let last_secs = last_at.duration_since(first_at).as_secs_f64();
    let hit = SignedDuration::try_from_secs_f64(hit_secs.max(last_secs))
        .ok()
        .and_then(|d| first_at.checked_add(d).ok());
    match hit {
        Some(at) if at <= resets_at => QuotaEta::WillHit { at },
        _ => QuotaEta::WontHit,
    }
}

/// Projected spend at the end of a period: `spent` plus the mean of the last
/// [`PROJECTION_DAYS`] daily amounts times the remaining days. With no history the
/// projection is `spent`.
///
/// # Errors
///
/// [`MoneyError::CurrencyMismatch`] when the history mixes currencies with `spent`.
pub fn project_period(
    spent: Money,
    daily_history: &[Money],
    remaining_days: u32,
) -> Result<Money, MoneyError> {
    let recent = &daily_history[daily_history.len().saturating_sub(PROJECTION_DAYS)..];
    let mut total: i128 = 0;
    for day in recent {
        spent.same_currency(*day)?;
        total += i128::from(day.micros());
    }
    let Ok(count) = i128::try_from(recent.len()) else {
        return Ok(spent);
    };
    if count == 0 {
        return Ok(spent);
    }
    // Multiply before dividing so the mean's remainder is not scaled by the remaining days.
    let future = total.saturating_mul(i128::from(remaining_days)) / count;
    let projected = i128::from(spent.micros()).saturating_add(future);
    Ok(Money::from_micros(
        spent.currency(),
        saturating_micros(projected),
    ))
}

/// Recent burn rate of one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRate {
    /// The agent.
    pub agent: AgentId,
    /// Tokens per minute over the window (input, cache and output tokens; reasoning is part of
    /// output).
    pub tokens_per_minute: u64,
    /// API-equivalent value per hour over the window; unpriced events count as zero.
    pub api_value_per_hour: UsdMicros,
    /// Distinct sessions with an event in the window.
    pub active_sessions: u32,
}

/// Running totals of one agent inside [`live_rate`].
#[derive(Default)]
struct Acc<'a> {
    tokens: u128,
    micros: i128,
    sessions: BTreeSet<&'a SessionId>,
}

/// Burn rate per agent over `(now - window, now]`, sorted by agent. Agents without events
/// in the window are omitted; a non-positive window yields nothing. Daily aggregates are not
/// live traffic and are ignored.
#[must_use]
pub fn live_rate(events: &[UsageEvent], now: Timestamp, window: SignedDuration) -> Vec<LiveRate> {
    let Ok(window_secs) = u128::try_from(window.as_secs()) else {
        return Vec::new();
    };
    if window_secs == 0 {
        return Vec::new();
    }
    let window_micros_divisor = i128::from(window.as_secs());
    let Ok(since) = now.checked_sub(window) else {
        return Vec::new();
    };
    let mut per_agent: BTreeMap<AgentId, Acc<'_>> = BTreeMap::new();
    for e in events
        .iter()
        .filter(|e| e.kind.is_traffic() && e.at > since && e.at <= now)
    {
        let acc = per_agent.entry(e.agent).or_default();
        acc.tokens += u128::from(e.tokens.total_tokens());
        acc.micros += i128::from(e.cost.api_cost().usd().map_or(0, UsdMicros::get));
        acc.sessions.insert(&e.session);
    }
    let div_round = |n: u128, d: u128| n.saturating_add(d / 2) / d;
    per_agent
        .into_iter()
        .map(|(agent, acc)| LiveRate {
            agent,
            tokens_per_minute: u64::try_from(div_round(acc.tokens.saturating_mul(60), window_secs))
                .unwrap_or(u64::MAX),
            api_value_per_hour: UsdMicros::new(saturating_micros(
                acc.micros
                    .saturating_mul(3600)
                    .saturating_add(window_micros_divisor / 2)
                    / window_micros_divisor,
            )),
            active_sessions: u32::try_from(acc.sessions.len()).unwrap_or(u32::MAX),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, TokenCounts};
    use crate::money::Currency;
    use crate::quota::{QuotaWindow, QuotaWindowState, Utilization};
    use crate::testing::{TestResult, usage_event};

    const RESET: &str = "2026-03-01T15:00:00Z";

    fn snap(
        at: &str,
        percent: f64,
        resets_at: Option<&str>,
    ) -> Result<QuotaSnapshot, Box<dyn std::error::Error>> {
        Ok(QuotaSnapshot {
            fetched_at: at.parse()?,
            windows: vec![QuotaWindow {
                id: WindowId::five_hour(),
                state: QuotaWindowState {
                    utilization: Utilization::new(percent)?,
                    resets_at: resets_at.map(str::parse).transpose()?,
                },
            }],
        })
    }

    fn eta(snaps: &[QuotaSnapshot]) -> Result<QuotaEta, Box<dyn std::error::Error>> {
        Ok(quota_eta(snaps, &WindowId::five_hour(), RESET.parse()?))
    }

    #[test]
    fn rising_window_hits_before_reset() -> TestResult {
        // +20% per 30 minutes from 20% at 12:00 reaches 100% at 14:00.
        let snaps = [
            snap("2026-03-01T12:00:00Z", 20.0, Some(RESET))?,
            snap("2026-03-01T12:30:00Z", 40.0, Some(RESET))?,
            snap("2026-03-01T13:00:00Z", 60.0, Some(RESET))?,
        ];
        let QuotaEta::WillHit { at } = eta(&snaps)? else {
            return Err("expected WillHit".into());
        };
        let expected: Timestamp = "2026-03-01T14:00:00Z".parse()?;
        assert!(at.duration_since(expected).abs() < SignedDuration::from_secs(1));
        Ok(())
    }

    #[test]
    fn slow_window_wont_hit() -> TestResult {
        let snaps = [
            snap("2026-03-01T12:00:00Z", 10.0, Some(RESET))?,
            snap("2026-03-01T12:30:00Z", 12.0, Some(RESET))?,
            snap("2026-03-01T13:00:00Z", 14.0, Some(RESET))?,
        ];
        assert_eq!(eta(&snaps)?, QuotaEta::WontHit);
        Ok(())
    }

    #[test]
    fn flat_or_falling_wont_hit() -> TestResult {
        let snaps = [
            snap("2026-03-01T12:00:00Z", 50.0, Some(RESET))?,
            snap("2026-03-01T12:30:00Z", 50.0, Some(RESET))?,
            snap("2026-03-01T13:00:00Z", 50.0, Some(RESET))?,
        ];
        assert_eq!(eta(&snaps)?, QuotaEta::WontHit);
        Ok(())
    }

    #[test]
    fn too_few_or_too_close_snapshots_are_not_enough() -> TestResult {
        assert_eq!(eta(&[])?, QuotaEta::NotEnoughData);
        let two = [
            snap("2026-03-01T12:00:00Z", 20.0, Some(RESET))?,
            snap("2026-03-01T13:00:00Z", 60.0, Some(RESET))?,
        ];
        assert_eq!(eta(&two)?, QuotaEta::NotEnoughData);
        let close = [
            snap("2026-03-01T12:00:00Z", 20.0, Some(RESET))?,
            snap("2026-03-01T12:05:00Z", 30.0, Some(RESET))?,
            snap("2026-03-01T12:10:00Z", 40.0, Some(RESET))?,
        ];
        assert_eq!(eta(&close)?, QuotaEta::NotEnoughData);
        Ok(())
    }

    #[test]
    fn snapshots_of_other_window_instances_are_ignored() -> TestResult {
        let snaps = [
            snap("2026-03-01T06:00:00Z", 90.0, Some("2026-03-01T10:00:00Z"))?,
            snap("2026-03-01T12:00:00Z", 20.0, Some(RESET))?,
            snap("2026-03-01T12:30:00Z", 40.0, Some(RESET))?,
            snap("2026-03-01T13:00:00Z", 60.0, None)?,
        ];
        assert_eq!(eta(&snaps)?, QuotaEta::NotEnoughData);
        Ok(())
    }

    #[test]
    fn exhausted_window_hits_now() -> TestResult {
        let snaps = [
            snap("2026-03-01T12:00:00Z", 80.0, Some(RESET))?,
            snap("2026-03-01T12:30:00Z", 95.0, Some(RESET))?,
            snap("2026-03-01T13:00:00Z", 100.0, Some(RESET))?,
        ];
        assert_eq!(
            eta(&snaps)?,
            QuotaEta::WillHit {
                at: "2026-03-01T13:00:00Z".parse()?
            }
        );
        Ok(())
    }

    fn usd(units: i64) -> Money {
        Money::from_micros(Currency::Usd, units * 1_000_000)
    }

    #[test]
    fn projection_uses_the_last_seven_days() -> TestResult {
        let mut history = vec![usd(1000)];
        history.extend((0..7).map(|_| usd(10)));
        assert_eq!(project_period(usd(100), &history, 10)?, usd(200));
        assert_eq!(project_period(usd(100), &[], 10)?, usd(100));
        assert_eq!(project_period(usd(100), &history, 0)?, usd(100));
        assert!(project_period(usd(1), &[Money::zero(Currency::Eur)], 1).is_err());
        Ok(())
    }

    fn event(
        agent: AgentId,
        session: &str,
        at: &str,
        tokens: u64,
        micros: i64,
    ) -> Result<UsageEvent, Box<dyn std::error::Error>> {
        let mut e = usage_event(agent, session, at)?;
        e.tokens = TokenCounts {
            output: tokens,
            ..TokenCounts::default()
        };
        e.cost = crate::event::EventCost::new(Some(UsdMicros::new(micros)), None);
        Ok(e)
    }

    #[test]
    fn live_rate_of_an_empty_window_is_empty() -> TestResult {
        let now: Timestamp = "2026-03-01T12:00:00Z".parse()?;
        assert_eq!(
            live_rate(&[], now, SignedDuration::from_mins(5)),
            Vec::new()
        );
        let old = event(AgentId::CLAUDE, "s", "2026-03-01T11:00:00Z", 100, 100)?;
        assert_eq!(
            live_rate(&[old], now, SignedDuration::from_mins(5)),
            Vec::new()
        );
        let e = event(AgentId::CLAUDE, "s", "2026-03-01T11:59:00Z", 100, 100)?;
        assert_eq!(live_rate(&[e], now, SignedDuration::ZERO), Vec::new());
        Ok(())
    }

    #[test]
    fn live_rate_of_one_session() -> TestResult {
        let now: Timestamp = "2026-03-01T12:00:00Z".parse()?;
        let events = [
            event(AgentId::CLAUDE, "s", "2026-03-01T11:58:00Z", 600, 50_000)?,
            event(AgentId::CLAUDE, "s", "2026-03-01T11:59:00Z", 400, 50_000)?,
        ];
        let rates = live_rate(&events, now, SignedDuration::from_mins(5));
        assert_eq!(
            rates,
            vec![LiveRate {
                agent: AgentId::CLAUDE,
                tokens_per_minute: 200,
                api_value_per_hour: UsdMicros::new(1_200_000),
                active_sessions: 1,
            }]
        );
        Ok(())
    }

    #[test]
    fn live_rate_splits_agents_and_ignores_aggregates() -> TestResult {
        let now: Timestamp = "2026-03-01T12:00:00Z".parse()?;
        let mut aggregate = event(AgentId::PI, "agg", "2026-03-01T11:59:00Z", 9_999, 9_999)?;
        aggregate.kind = EventKind::DailyAggregate;
        let events = [
            event(AgentId::PI, "p1", "2026-03-01T11:59:00Z", 300, 0)?,
            event(AgentId::CLAUDE, "c1", "2026-03-01T11:59:00Z", 100, 0)?,
            event(AgentId::CLAUDE, "c2", "2026-03-01T11:59:30Z", 100, 0)?,
            aggregate,
        ];
        let rates = live_rate(&events, now, SignedDuration::from_mins(10));
        assert_eq!(rates.len(), 2);
        let by = |agent| rates.iter().find(|r| r.agent == agent);
        assert_eq!(
            by(AgentId::CLAUDE).map(|r| (r.tokens_per_minute, r.active_sessions)),
            Some((20, 2))
        );
        assert_eq!(
            by(AgentId::PI).map(|r| (r.tokens_per_minute, r.active_sessions)),
            Some((30, 1))
        );
        Ok(())
    }
}
