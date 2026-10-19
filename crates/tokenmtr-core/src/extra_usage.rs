//! Claude "extra usage": pay-as-you-go spend beyond a plan's quota windows.
//!
//! The quota endpoint reports the current month's spend and cap in US cents (Superset reads
//! them the same way). A poll stores them as the [`EXTRA_USAGE`] quota window with money, and the
//! spend increase since the previous poll as one [`EventKind::ExtraUsage`] usage event per agent,
//! profile and local day, so paid cost counts it.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;

use crate::event::{EventCost, EventKind, TokenCounts, UsageEvent};
use crate::ids::{AgentId, DedupKey, EventId, IdError, MachineId, ModelId, ProfileId, SessionId};
use crate::money::{MoneyError, UsdMicros};
use crate::quota::{BadUtilization, QuotaWindow, QuotaWindowState, Utilization, WindowId};

/// The window id, model, session and event-kind text of extra usage.
pub const EXTRA_USAGE: &str = "extra_usage";

/// Why an extra-usage amount was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BadExtraUsage {
    /// An amount was negative.
    #[error("extra-usage amount is negative")]
    Negative,
    /// An amount was not finite or overflowed.
    #[error(transparent)]
    Money(#[from] MoneyError),
    /// The computed utilization was not a valid percentage.
    #[error("extra-usage utilization is invalid")]
    Utilization,
}

impl From<BadUtilization> for BadExtraUsage {
    fn from(_: BadUtilization) -> Self {
        Self::Utilization
    }
}

/// The extra usage of one account for the current month, in USD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtraUsage {
    /// Percent of the cap spent; may exceed 100.
    pub utilization: Utilization,
    /// Spent this month.
    pub spent: UsdMicros,
    /// Monthly cap.
    pub limit: UsdMicros,
}

impl ExtraUsage {
    /// Builds the month's extra usage from amounts in US cents (fractional allowed).
    /// `utilization` is the provider's percentage; without it, it is `used / limit`, and 100 when
    /// a zero cap is overspent.
    ///
    /// # Errors
    ///
    /// [`BadExtraUsage`] for a negative, non-finite or overflowing amount.
    pub fn from_cents(
        used_cents: f64,
        limit_cents: f64,
        utilization: Option<Utilization>,
    ) -> Result<Self, BadExtraUsage> {
        let spent = usd_from_cents(used_cents)?;
        let limit = usd_from_cents(limit_cents)?;
        let utilization = match utilization {
            Some(utilization) => utilization,
            None if limit_cents > 0.0 => Utilization::new(100.0 * used_cents / limit_cents)?,
            None if used_cents > 0.0 => Utilization::new(100.0)?,
            None => Utilization::new(0.0)?,
        };
        Ok(Self {
            utilization,
            spent,
            limit,
        })
    }

    /// The [`EXTRA_USAGE`] window id.
    #[must_use]
    pub fn window_id() -> WindowId {
        WindowId::extra_usage()
    }

    /// The month as a quota window: no reset instant, the provider does not report one.
    #[must_use]
    pub fn window(&self) -> QuotaWindow {
        QuotaWindow {
            id: Self::window_id(),
            state: QuotaWindowState {
                utilization: self.utilization,
                resets_at: None,
            },
        }
    }
}

/// US cents (a float at the provider boundary) as USD micros: cents × 10 000, rounded half away
/// from zero.
///
/// # Errors
///
/// [`BadExtraUsage`] for a negative, non-finite or overflowing amount.
pub fn usd_from_cents(cents: f64) -> Result<UsdMicros, BadExtraUsage> {
    if !cents.is_finite() {
        return Err(MoneyError::NotFinite.into());
    }
    if cents < 0.0 {
        return Err(BadExtraUsage::Negative);
    }
    let cents = Decimal::from_f64(cents).ok_or(MoneyError::OutOfRange)?;
    let dollars = cents
        .checked_div(Decimal::ONE_HUNDRED)
        .ok_or(MoneyError::OutOfRange)?;
    Ok(UsdMicros::from_decimal(dollars)?)
}

/// What one poll adds to the extra-usage spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpendIncrease {
    /// The increase.
    pub amount: UsdMicros,
    /// True on the first observation: the whole month so far lands on the poll's day.
    pub approximate: bool,
}

/// The spend added since the `previous` poll of the same agent profile. A lower spend means
/// the monthly cycle restarted, so the whole current spend is new. Without a previous poll the
/// whole spend counts, flagged approximate.
#[must_use]
pub fn spend_increase(previous: Option<UsdMicros>, spent: UsdMicros) -> SpendIncrease {
    match previous {
        None => SpendIncrease {
            amount: spent,
            approximate: true,
        },
        Some(previous) if spent < previous => SpendIncrease {
            amount: spent,
            approximate: false,
        },
        Some(previous) => SpendIncrease {
            amount: UsdMicros::new(spent.get().saturating_sub(previous.get())),
            approximate: false,
        },
    }
}

/// The dedup key of the extra-usage event of `profile` on local `day`. Permanent.
///
/// # Errors
///
/// Never in practice: the key is prefixed, so it is never blank.
pub fn daily_dedup_key(profile: &ProfileId, day: Date) -> Result<DedupKey, IdError> {
    DedupKey::try_from(format!("{EXTRA_USAGE}|{profile}|{day}"))
}

/// Why the extra-usage event of a day could not be built.
#[derive(Debug, thiserror::Error)]
pub enum DailyEventError {
    /// An id was rejected.
    #[error(transparent)]
    Id(#[from] IdError),
    /// The day's noon does not exist in the zone's supported range.
    #[error(transparent)]
    Time(#[from] jiff::Error),
}

/// The extra-usage event of `agent`/`profile` on local `day`: zero tokens, the day's spend as a
/// reported cost, at noon in `zone` like stats-cache daily aggregates, and no project or session
/// data.
///
/// # Errors
///
/// [`DailyEventError`] when the day's noon is out of range.
pub fn daily_event(
    machine: &MachineId,
    agent: AgentId,
    profile: ProfileId,
    day: Date,
    zone: &TimeZone,
    spent: UsdMicros,
    approximate: bool,
) -> Result<UsageEvent, DailyEventError> {
    let dedup_key = daily_dedup_key(&profile, day)?;
    Ok(UsageEvent {
        id: EventId::new(machine, agent, &dedup_key),
        agent,
        profile,
        dedup_key,
        at: day.at(12, 0, 0, 0).to_zoned(zone.clone())?.timestamp(),
        session: SessionId::try_from(EXTRA_USAGE)?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        project: None,
        client: None,
        source_file: None,
        approximate,
        provider: None,
        model: ModelId::try_from(EXTRA_USAGE)?,
        kind: EventKind::ExtraUsage,
        tokens: TokenCounts::default(),
        cost: EventCost::new(Some(spent), None),
        service_tier: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn cents_become_usd_micros() -> TestResult {
        assert_eq!(usd_from_cents(1234.5)?, UsdMicros::new(12_345_000));
        assert_eq!(usd_from_cents(5000.0)?, UsdMicros::new(50_000_000));
        assert_eq!(usd_from_cents(0.00005)?, UsdMicros::new(1));
        assert_eq!(usd_from_cents(-1.0), Err(BadExtraUsage::Negative));
        assert_eq!(
            usd_from_cents(f64::NAN),
            Err(BadExtraUsage::Money(MoneyError::NotFinite))
        );
        assert!(usd_from_cents(f64::INFINITY).is_err());
        assert!(usd_from_cents(1e300).is_err());
        Ok(())
    }

    #[test]
    fn utilization_is_reported_or_computed() -> TestResult {
        let reported = ExtraUsage::from_cents(1234.5, 5000.0, Some(Utilization::new(24.69)?))?;
        assert!((reported.utilization.percent() - 24.69).abs() < 1e-9);
        assert_eq!(reported.limit, UsdMicros::new(50_000_000));
        let computed = ExtraUsage::from_cents(1250.0, 5000.0, None)?;
        assert!((computed.utilization.percent() - 25.0).abs() < 1e-9);
        let no_cap = ExtraUsage::from_cents(10.0, 0.0, None)?;
        assert!((no_cap.utilization.percent() - 100.0).abs() < 1e-9);
        assert_eq!(computed.window().id.as_str(), EXTRA_USAGE);
        assert_eq!(computed.window().state.resets_at, None);
        assert!(ExtraUsage::from_cents(-1.0, 5000.0, None).is_err());
        Ok(())
    }

    #[test]
    fn increase_handles_first_observation_growth_and_monthly_reset() {
        let usd = UsdMicros::new;
        assert_eq!(
            spend_increase(None, usd(500)),
            SpendIncrease {
                amount: usd(500),
                approximate: true
            }
        );
        assert_eq!(
            spend_increase(Some(usd(500)), usd(800)),
            SpendIncrease {
                amount: usd(300),
                approximate: false
            }
        );
        assert_eq!(spend_increase(Some(usd(800)), usd(800)).amount, usd(0));
        assert_eq!(
            spend_increase(Some(usd(800)), usd(100)),
            SpendIncrease {
                amount: usd(100),
                approximate: false
            }
        );
    }

    #[test]
    fn daily_event_sits_at_local_noon_with_a_reported_cost() -> TestResult {
        let machine: MachineId = "0190a6f2-7c1e-7b4a-9d3e-1a2b3c4d5e6f".parse()?;
        let zone = TimeZone::get("Europe/Paris")?;
        let day: Date = "2026-03-10".parse()?;
        let event = daily_event(
            &machine,
            AgentId::CLAUDE,
            ProfileId::default_profile(),
            day,
            &zone,
            UsdMicros::new(42),
            true,
        )?;
        assert_eq!(event.at.to_string(), "2026-03-10T11:00:00Z");
        assert_eq!(event.dedup_key.as_str(), "extra_usage|default|2026-03-10");
        assert_eq!(event.kind, EventKind::ExtraUsage);
        assert_eq!(event.cost.reported(), Some(UsdMicros::new(42)));
        assert_eq!(event.tokens, TokenCounts::default());
        assert!(event.approximate);
        Ok(())
    }
}
