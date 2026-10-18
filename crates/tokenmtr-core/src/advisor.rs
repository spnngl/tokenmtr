//! Plan advisor (D35): which subscription plan fits the observed quota usage.

use jiff::SignedDuration;
use serde::{Deserialize, Serialize};

use crate::billing::{Plan, PlanCatalog};
use crate::money::UsdMicros;
use crate::quota::{QuotaSnapshot, WindowId};

/// How far back the advisor looks.
pub const ADVISOR_WINDOW: SignedDuration = SignedDuration::from_hours(30 * 24);
/// Minimum span of snapshots before the advisor says anything.
pub const MIN_ADVISOR_SPAN: SignedDuration = SignedDuration::from_hours(14 * 24);
/// Default percentage of quota a plan may use at peak and still be considered a fit.
pub const DEFAULT_HEADROOM: u8 = 80;

/// What the advisor suggests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict", content = "plan")]
pub enum Verdict {
    /// The current plan is the cheapest that fits.
    Keep,
    /// A cheaper plan would still fit.
    Downgrade(Plan),
    /// The current plan is too small; this one is the cheapest that fits (or the largest there is).
    Upgrade(Plan),
    /// API rates would cost less than the cheapest fitting plan.
    PayAsYouGo,
}

/// Peak utilization a plan would have seen over the advisor window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanFit {
    /// The plan.
    pub plan: Plan,
    /// Its list monthly fee.
    pub monthly_fee: UsdMicros,
    /// Peak five-hour utilization, rescaled to this plan's quota.
    pub five_hour_peak: f64,
    /// Peak seven-day utilization, rescaled to this plan's quota.
    pub seven_day_peak: f64,
    /// Both peaks are within the headroom.
    pub fits: bool,
}

/// The advisor's reasoning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    /// The suggestion.
    pub verdict: Verdict,
    /// The plan the user is on.
    pub current: Plan,
    /// Headroom used, in percent.
    pub headroom: u8,
    /// Every catalog plan, cheapest first.
    pub fits: Vec<PlanFit>,
}

/// The advisor's output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "advice")]
pub enum Advice {
    /// The snapshots span fewer than 14 days.
    NotEnoughData {
        /// Whole days spanned by the snapshots in the window.
        span_days: u32,
    },
    /// The current plan has no quota multiplier in the catalog (team, enterprise, custom).
    NotApplicable,
    /// A recommendation.
    Recommend(Recommendation),
}

/// Recommends a plan from the last 30 days of quota snapshots (D35).
///
/// Each window kind's peak is rescaled from the `current` plan to every catalog plan by the ratio
/// of their quota multipliers; the cheapest plan whose peaks are both within `headroom` percent
/// wins (the largest plan when none fits). If `api_value` (what that period would have cost at
/// API rates, scaled to a month by the caller) is below the winner's monthly fee, pay-as-you-go
/// is advised instead.
#[must_use]
pub fn advise(
    snapshots: &[QuotaSnapshot],
    catalog: &PlanCatalog,
    current: Plan,
    headroom: u8,
    api_value: UsdMicros,
) -> Advice {
    let Some(current_spec) = catalog.get(current) else {
        return Advice::NotApplicable;
    };
    let Some(latest) = snapshots.iter().map(|s| s.fetched_at).max() else {
        return Advice::NotEnoughData { span_days: 0 };
    };
    let Ok(since) = latest.checked_sub(ADVISOR_WINDOW) else {
        return Advice::NotEnoughData { span_days: 0 };
    };
    let recent: Vec<&QuotaSnapshot> = snapshots.iter().filter(|s| s.fetched_at >= since).collect();
    let earliest = recent.iter().map(|s| s.fetched_at).min().unwrap_or(latest);
    let span = latest.duration_since(earliest);
    if span < MIN_ADVISOR_SPAN {
        let days = span.as_secs() / (24 * 3600);
        return Advice::NotEnoughData {
            span_days: u32::try_from(days).unwrap_or(0),
        };
    }

    let peak = |id: &WindowId| {
        recent
            .iter()
            .filter_map(|s| s.window(id))
            .map(|w| w.utilization.percent())
            .fold(0.0_f64, f64::max)
    };
    let (five, seven) = (peak(&WindowId::five_hour()), peak(&WindowId::seven_day()));

    let headroom = headroom.min(100);
    let mut specs: Vec<_> = catalog.plans().iter().collect();
    specs.sort_by_key(|p| (p.monthly_fee, p.quota_multiplier));
    let fits: Vec<PlanFit> = specs
        .iter()
        .map(|spec| {
            let scale = f64::from(current_spec.quota_multiplier) / f64::from(spec.quota_multiplier);
            let (five_hour_peak, seven_day_peak) = (five * scale, seven * scale);
            let limit = f64::from(headroom);
            PlanFit {
                plan: spec.plan,
                monthly_fee: spec.monthly_fee,
                five_hour_peak,
                seven_day_peak,
                fits: five_hour_peak <= limit && seven_day_peak <= limit,
            }
        })
        .collect();

    let Some(chosen) = fits.iter().find(|f| f.fits).or(fits.last()) else {
        return Advice::NotApplicable;
    };
    let verdict = if api_value < chosen.monthly_fee {
        Verdict::PayAsYouGo
    } else if chosen.plan == current {
        Verdict::Keep
    } else if chosen.monthly_fee < current_spec.monthly_fee {
        Verdict::Downgrade(chosen.plan)
    } else {
        Verdict::Upgrade(chosen.plan)
    };
    Advice::Recommend(Recommendation {
        verdict,
        current,
        headroom,
        fits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::{QuotaWindow, QuotaWindowState, Utilization};
    use crate::testing::TestResult;
    use jiff::Timestamp;

    /// One snapshot per day for `days + 1` days; day 3 carries the peaks, other days half.
    fn history(
        days: i64,
        five: f64,
        seven: f64,
    ) -> Result<Vec<QuotaSnapshot>, Box<dyn std::error::Error>> {
        let start: Timestamp = "2026-02-01T00:00:00Z".parse()?;
        (0..=days)
            .map(|d| {
                let scale = if d == 3 { 1.0 } else { 0.5 };
                Ok(QuotaSnapshot {
                    fetched_at: start.checked_add(SignedDuration::from_hours(24 * d))?,
                    windows: vec![
                        QuotaWindow {
                            id: WindowId::five_hour(),
                            state: QuotaWindowState {
                                utilization: Utilization::new(five * scale)?,
                                resets_at: None,
                            },
                        },
                        QuotaWindow {
                            id: WindowId::seven_day(),
                            state: QuotaWindowState {
                                utilization: Utilization::new(seven * scale)?,
                                resets_at: None,
                            },
                        },
                    ],
                })
            })
            .collect()
    }

    fn verdict(advice: Advice) -> Result<Verdict, Box<dyn std::error::Error>> {
        match advice {
            Advice::Recommend(r) => Ok(r.verdict),
            other => Err(format!("expected a recommendation, got {other:?}").into()),
        }
    }

    fn run(
        snaps: &[QuotaSnapshot],
        current: Plan,
        api_dollars: i64,
    ) -> Result<Advice, Box<dyn std::error::Error>> {
        Ok(advise(
            snaps,
            &PlanCatalog::embedded()?,
            current,
            DEFAULT_HEADROOM,
            UsdMicros::new(api_dollars * 1_000_000),
        ))
    }

    #[test]
    fn light_usage_on_max20x_downgrades() -> TestResult {
        // Max5x would see 20% / 12%; Pro would see 100%.
        let snaps = history(20, 5.0, 3.0)?;
        assert_eq!(
            verdict(run(&snaps, Plan::Max20x, 5_000)?)?,
            Verdict::Downgrade(Plan::Max5x)
        );
        Ok(())
    }

    #[test]
    fn heavy_usage_on_pro_upgrades() -> TestResult {
        let snaps = history(20, 100.0, 60.0)?;
        assert_eq!(
            verdict(run(&snaps, Plan::Pro, 5_000)?)?,
            Verdict::Upgrade(Plan::Max5x)
        );
        Ok(())
    }

    #[test]
    fn usage_beyond_every_plan_keeps_the_largest() -> TestResult {
        let snaps = history(20, 100.0, 100.0)?;
        assert_eq!(verdict(run(&snaps, Plan::Max20x, 50_000)?)?, Verdict::Keep);
        Ok(())
    }

    #[test]
    fn well_fitted_plan_is_kept() -> TestResult {
        let snaps = history(20, 70.0, 50.0)?;
        assert_eq!(verdict(run(&snaps, Plan::Max5x, 5_000)?)?, Verdict::Keep);
        Ok(())
    }

    #[test]
    fn pay_as_you_go_when_api_is_cheaper_than_the_cheapest_fit() -> TestResult {
        let snaps = history(20, 5.0, 3.0)?;
        // Pro fits (Max5x scaled by 5); API value $10 < Pro's $20.
        assert_eq!(verdict(run(&snaps, Plan::Max5x, 10)?)?, Verdict::PayAsYouGo);
        Ok(())
    }

    #[test]
    fn fewer_than_fourteen_days_is_not_enough() -> TestResult {
        let snaps = history(10, 5.0, 3.0)?;
        assert_eq!(
            run(&snaps, Plan::Max5x, 100)?,
            Advice::NotEnoughData { span_days: 10 }
        );
        assert_eq!(
            run(&[], Plan::Max5x, 100)?,
            Advice::NotEnoughData { span_days: 0 }
        );
        Ok(())
    }

    #[test]
    fn only_the_last_thirty_days_count() -> TestResult {
        // 60 days of history: the spike on day 3 is outside the window.
        let snaps = history(60, 100.0, 100.0)?;
        let Advice::Recommend(r) = run(&snaps, Plan::Max20x, 50_000)? else {
            return Err("expected a recommendation".into());
        };
        let fit = |p| {
            r.fits
                .iter()
                .find(|f| f.plan == p)
                .map(|f| f.five_hour_peak)
        };
        assert_eq!(fit(Plan::Max20x), Some(50.0));
        Ok(())
    }

    #[test]
    fn plans_without_a_multiplier_are_not_applicable() -> TestResult {
        let snaps = history(20, 5.0, 3.0)?;
        assert_eq!(run(&snaps, Plan::Team, 100)?, Advice::NotApplicable);
        Ok(())
    }
}
