//! Alert rules: pure evaluation of what should fire, deduplicated by key.

use std::collections::HashSet;

use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::forecast::RESET_TOLERANCE;
use crate::ids::{AgentId, BudgetId, ProfileId, SinkId};
use crate::quota::{Utilization, WindowId};

/// What an alert is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AlertKind {
    /// A quota window crossed a utilization threshold.
    QuotaThreshold {
        /// The window.
        window: WindowId,
        /// The threshold crossed, in percent.
        percent: u8,
    },
    /// A quota window reset.
    QuotaReset {
        /// The window.
        window: WindowId,
    },
    /// A budget crossed a threshold.
    BudgetThreshold {
        /// The budget.
        budget: BudgetId,
        /// The threshold crossed, in percent.
        percent: u8,
    },
    /// A sink has been failing for longer than allowed.
    SinkFailing {
        /// The sink.
        sink: SinkId,
        /// When it started failing.
        since: Timestamp,
    },
    /// An agent's log format drifted from what the parser knows.
    FormatDrift {
        /// The agent.
        agent: AgentId,
    },
    /// The last backup failed.
    BackupFailed,
}

/// The `[alerts]` section of the config, as far as the rules need it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertConfig {
    /// Quota utilization thresholds, in percent.
    pub quota_thresholds: Vec<u8>,
    /// Whether to announce quota resets.
    pub quota_reset: bool,
    /// Budget thresholds, in percent.
    pub budget_thresholds: Vec<u8>,
    /// How long a sink may fail before alerting.
    pub sink_failing_after: SignedDuration,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            quota_thresholds: vec![80, 95],
            quota_reset: true,
            budget_thresholds: vec![80, 100],
            sink_failing_after: SignedDuration::from_mins(30),
        }
    }
}

/// Latest known state of one quota window of one profile.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindowStatus {
    /// The agent whose quota this is.
    pub agent: AgentId,
    /// The profile whose quota this is; each profile has its own windows.
    pub profile: ProfileId,
    /// The window.
    pub window: WindowId,
    /// Current utilization.
    pub utilization: Utilization,
    /// When the current window instance resets, as reported. It identifies the instance; the
    /// rules snap it to [`RESET_TOLERANCE`] so jitter between polls keeps the same instance.
    /// Without it thresholds cannot be deduplicated per window.
    pub resets_at: Option<Timestamp>,
    /// The `resets_at` of the instance seen at the previous poll.
    pub previous_resets_at: Option<Timestamp>,
}

/// Latest known state of one budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetState {
    /// The budget.
    pub id: BudgetId,
    /// Percent of the limit spent (whole percent).
    pub percent: u32,
    /// First day of the current budget period; identifies the occurrence.
    pub period_start: Date,
}

/// A sink's failure state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkState {
    /// The sink.
    pub id: SinkId,
    /// Since when it has been failing; `None` when healthy.
    pub failing_since: Option<Timestamp>,
}

/// An open format-drift issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftState {
    /// The agent whose logs drifted.
    pub agent: AgentId,
    /// When the issue was first seen; identifies the occurrence.
    pub opened_at: Timestamp,
}

/// Everything the rules look at.
#[derive(Debug, Clone)]
pub struct AlertInputs {
    /// Evaluation time.
    pub now: Timestamp,
    /// Quota windows.
    pub quota: Vec<QuotaWindowStatus>,
    /// Budgets.
    pub budgets: Vec<BudgetState>,
    /// Sinks.
    pub sinks: Vec<SinkState>,
    /// Open drift issues.
    pub drift: Vec<DriftState>,
    /// When the last backup failed, if it did.
    pub backup_failed_at: Option<Timestamp>,
    /// Dedup keys of alerts already fired (from the store).
    pub already_fired: HashSet<String>,
}

impl AlertInputs {
    /// Inputs at `now` with nothing to report.
    #[must_use]
    pub fn new(now: Timestamp) -> Self {
        Self {
            now,
            quota: Vec::new(),
            budgets: Vec::new(),
            sinks: Vec::new(),
            drift: Vec::new(),
            backup_failed_at: None,
            already_fired: HashSet::new(),
        }
    }
}

/// An alert to record and deliver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    /// What it is about.
    pub kind: AlertKind,
    /// Unique per occurrence; the store refuses duplicates.
    pub dedup_key: String,
    /// When it fired.
    pub at: Timestamp,
}

/// `resets_at` rounded to the nearest multiple of [`RESET_TOLERANCE`] since the epoch, so the
/// few seconds of jitter an API adds between polls name the same window instance. Resets fall on
/// round times, far from the rounding midpoints.
fn snap(resets_at: Timestamp) -> Timestamp {
    let width = RESET_TOLERANCE.as_secs();
    let snapped = resets_at
        .as_second()
        .saturating_add(width / 2)
        .div_euclid(width)
        .saturating_mul(width);
    Timestamp::from_second(snapped).unwrap_or(resets_at)
}

fn instance(resets_at: Option<Timestamp>) -> String {
    resets_at.map_or_else(|| "-".to_owned(), |t| snap(t).to_string())
}

/// Whether `resets_at` names another window instance than `previous`.
fn is_new_instance(resets_at: Option<Timestamp>, previous: Timestamp) -> bool {
    resets_at.is_none_or(|t| t.duration_since(previous).abs() > RESET_TOLERANCE)
}

/// The alerts that should fire now, skipping dedup keys in `already_fired`.
///
/// Each quota threshold fires once per agent, profile and window instance and again after the
/// window resets (the instance, identified by the snapped `resets_at`, changes); budgets likewise per period; a sink alert once
/// per failure streak; drift once per issue; backup failure once per failure time. When one
/// evaluation crosses several thresholds, each fires. Output order is deterministic: quota,
/// budgets, sinks, drift, backup.
#[must_use]
pub fn evaluate_alerts(inputs: &AlertInputs, config: &AlertConfig) -> Vec<Alert> {
    let now = inputs.now;
    let mut out = Vec::new();
    let mut fire = |kind: AlertKind, key: String| {
        if !inputs.already_fired.contains(&key) {
            out.push(Alert {
                kind,
                dedup_key: key,
                at: now,
            });
        }
    };

    let mut quota_thresholds = config.quota_thresholds.clone();
    quota_thresholds.sort_unstable();
    quota_thresholds.dedup();
    for q in &inputs.quota {
        for &percent in &quota_thresholds {
            if q.utilization.percent() >= f64::from(percent) {
                fire(
                    AlertKind::QuotaThreshold {
                        window: q.window.clone(),
                        percent,
                    },
                    format!(
                        "quota_threshold|{}|{}|{}|{percent}|{}",
                        q.agent.as_str(),
                        q.profile,
                        q.window,
                        instance(q.resets_at)
                    ),
                );
            }
        }
        if let Some(previous) = q.previous_resets_at
            && config.quota_reset
            && previous <= now
            && is_new_instance(q.resets_at, previous)
        {
            fire(
                AlertKind::QuotaReset {
                    window: q.window.clone(),
                },
                format!(
                    "quota_reset|{}|{}|{}|{}",
                    q.agent.as_str(),
                    q.profile,
                    q.window,
                    instance(Some(previous))
                ),
            );
        }
    }

    let mut budget_thresholds = config.budget_thresholds.clone();
    budget_thresholds.sort_unstable();
    budget_thresholds.dedup();
    for b in &inputs.budgets {
        for &percent in &budget_thresholds {
            if b.percent >= u32::from(percent) {
                fire(
                    AlertKind::BudgetThreshold {
                        budget: b.id.clone(),
                        percent,
                    },
                    format!("budget_threshold|{}|{percent}|{}", b.id, b.period_start),
                );
            }
        }
    }

    for s in &inputs.sinks {
        if let Some(since) = s.failing_since
            && since
                .checked_add(config.sink_failing_after)
                .is_ok_and(|due| due <= now)
        {
            fire(
                AlertKind::SinkFailing {
                    sink: s.id.clone(),
                    since,
                },
                format!("sink_failing|{}|{since}", s.id),
            );
        }
    }

    for d in &inputs.drift {
        fire(
            AlertKind::FormatDrift { agent: d.agent },
            format!("format_drift|{}|{}", d.agent.as_str(), d.opened_at),
        );
    }

    if let Some(at) = inputs.backup_failed_at {
        fire(AlertKind::BackupFailed, format!("backup_failed|{at}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestResult;

    const RESET_1: &str = "2026-03-01T15:00:00Z";
    const RESET_2: &str = "2026-03-01T20:00:00Z";

    fn quota(
        percent: f64,
        resets_at: &str,
        previous: Option<&str>,
    ) -> Result<QuotaWindowStatus, Box<dyn std::error::Error>> {
        Ok(QuotaWindowStatus {
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            window: WindowId::five_hour(),
            utilization: Utilization::new(percent)?,
            resets_at: Some(resets_at.parse()?),
            previous_resets_at: previous.map(str::parse).transpose()?,
        })
    }

    fn inputs(
        now: &str,
        quota: QuotaWindowStatus,
        fired: &[Alert],
    ) -> Result<AlertInputs, Box<dyn std::error::Error>> {
        Ok(AlertInputs {
            now: now.parse()?,
            quota: vec![quota],
            already_fired: fired.iter().map(|a| a.dedup_key.clone()).collect(),
            ..AlertInputs::new(Timestamp::UNIX_EPOCH)
        })
    }

    fn percents(alerts: &[Alert]) -> Vec<u8> {
        alerts
            .iter()
            .filter_map(|a| match a.kind {
                AlertKind::QuotaThreshold { percent, .. } => Some(percent),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_quota_threshold_fires_once_per_window_and_again_after_reset() -> TestResult {
        let config = AlertConfig::default();
        let mut fired: Vec<Alert> = Vec::new();

        let a = evaluate_alerts(
            &inputs("2026-03-01T12:00:00Z", quota(50.0, RESET_1, None)?, &fired)?,
            &config,
        );
        assert_eq!(a, Vec::new());

        let a = evaluate_alerts(
            &inputs("2026-03-01T12:10:00Z", quota(85.0, RESET_1, None)?, &fired)?,
            &config,
        );
        assert_eq!(percents(&a), vec![80]);
        fired.extend(a);

        let a = evaluate_alerts(
            &inputs("2026-03-01T12:20:00Z", quota(90.0, RESET_1, None)?, &fired)?,
            &config,
        );
        assert_eq!(a, Vec::new());

        let a = evaluate_alerts(
            &inputs("2026-03-01T12:30:00Z", quota(96.0, RESET_1, None)?, &fired)?,
            &config,
        );
        assert_eq!(percents(&a), vec![95]);
        fired.extend(a);

        let a = evaluate_alerts(
            &inputs("2026-03-01T12:40:00Z", quota(99.0, RESET_1, None)?, &fired)?,
            &config,
        );
        assert_eq!(a, Vec::new());

        // The window reset: new instance, thresholds fire again, and the reset is announced once.
        let a = evaluate_alerts(
            &inputs(
                "2026-03-01T15:05:00Z",
                quota(82.0, RESET_2, Some(RESET_1))?,
                &fired,
            )?,
            &config,
        );
        assert_eq!(percents(&a), vec![80]);
        assert!(
            a.iter()
                .any(|x| matches!(x.kind, AlertKind::QuotaReset { .. }))
        );
        fired.extend(a);
        let a = evaluate_alerts(
            &inputs(
                "2026-03-01T15:10:00Z",
                quota(82.0, RESET_2, Some(RESET_1))?,
                &fired,
            )?,
            &config,
        );
        assert_eq!(a, Vec::new());
        Ok(())
    }

    #[test]
    fn a_jump_across_thresholds_fires_each_once() -> TestResult {
        let a = evaluate_alerts(
            &inputs("2026-03-01T12:00:00Z", quota(97.0, RESET_1, None)?, &[])?,
            &AlertConfig::default(),
        );
        assert_eq!(percents(&a), vec![80, 95]);
        Ok(())
    }

    #[test]
    fn reset_jitter_keeps_the_window_instance() -> TestResult {
        let config = AlertConfig::default();
        let first = evaluate_alerts(
            &inputs("2026-03-01T12:00:00Z", quota(85.0, RESET_1, None)?, &[])?,
            &config,
        );
        assert_eq!(percents(&first), vec![80]);
        let jittered = quota(85.0, "2026-03-01T15:00:01.4Z", Some("2026-03-01T14:59:59Z"))?;
        let a = evaluate_alerts(&inputs("2026-03-01T15:00:30Z", jittered, &first)?, &config);
        assert_eq!(a, Vec::new());
        Ok(())
    }

    #[test]
    fn each_profile_has_its_own_thresholds() -> TestResult {
        let config = AlertConfig::default();
        let first = evaluate_alerts(
            &inputs("2026-03-01T12:00:00Z", quota(85.0, RESET_1, None)?, &[])?,
            &config,
        );
        let mut work = quota(85.0, RESET_1, None)?;
        work.profile = ProfileId::try_from("work")?;
        let a = evaluate_alerts(&inputs("2026-03-01T12:00:00Z", work, &first)?, &config);
        assert_eq!(percents(&a), vec![80]);
        Ok(())
    }

    #[test]
    fn reset_is_not_announced_before_it_is_due_or_when_disabled() -> TestResult {
        let config = AlertConfig::default();
        // resets_at jittered but the previous instance is still in the future.
        let a = evaluate_alerts(
            &inputs(
                "2026-03-01T14:00:00Z",
                quota(10.0, RESET_2, Some(RESET_1))?,
                &[],
            )?,
            &config,
        );
        assert_eq!(a, Vec::new());
        let off = AlertConfig {
            quota_reset: false,
            ..AlertConfig::default()
        };
        let a = evaluate_alerts(
            &inputs(
                "2026-03-01T15:05:00Z",
                quota(10.0, RESET_2, Some(RESET_1))?,
                &[],
            )?,
            &off,
        );
        assert_eq!(a, Vec::new());
        Ok(())
    }

    #[test]
    fn budgets_fire_once_per_period() -> TestResult {
        let state = |percent, day| -> Result<BudgetState, Box<dyn std::error::Error>> {
            Ok(BudgetState {
                id: BudgetId::try_from("b")?,
                percent,
                period_start: jiff::civil::date(2026, 3, day),
            })
        };
        let run =
            |s: BudgetState, fired: &[Alert]| -> Result<Vec<Alert>, Box<dyn std::error::Error>> {
                Ok(evaluate_alerts(
                    &AlertInputs {
                        now: "2026-03-10T00:00:00Z".parse()?,
                        budgets: vec![s],
                        already_fired: fired.iter().map(|a| a.dedup_key.clone()).collect(),
                        ..AlertInputs::new(Timestamp::UNIX_EPOCH)
                    },
                    &AlertConfig::default(),
                ))
            };
        let first = run(state(100, 1)?, &[])?;
        assert_eq!(first.len(), 2);
        assert_eq!(run(state(130, 1)?, &first)?, Vec::new());
        assert_eq!(run(state(81, 8)?, &first)?.len(), 1);
        assert_eq!(run(state(79, 8)?, &first)?, Vec::new());
        Ok(())
    }

    #[test]
    fn sinks_alert_only_after_the_grace_period() -> TestResult {
        let since: Timestamp = "2026-03-01T12:00:00Z".parse()?;
        let run = |now: &str| -> Result<Vec<Alert>, Box<dyn std::error::Error>> {
            Ok(evaluate_alerts(
                &AlertInputs {
                    now: now.parse()?,
                    sinks: vec![SinkState {
                        id: SinkId::try_from("csv")?,
                        failing_since: Some(since),
                    }],
                    ..AlertInputs::new(Timestamp::UNIX_EPOCH)
                },
                &AlertConfig::default(),
            ))
        };
        assert_eq!(run("2026-03-01T12:29:59Z")?, Vec::new());
        let a = run("2026-03-01T12:30:00Z")?;
        assert_eq!(a.len(), 1);
        assert!(matches!(&a[0].kind, AlertKind::SinkFailing { since: s, .. } if *s == since));
        Ok(())
    }

    #[test]
    fn drift_and_backup_failures_deduplicate() -> TestResult {
        let now: Timestamp = "2026-03-01T12:00:00Z".parse()?;
        let mut inputs = AlertInputs {
            now,
            drift: vec![DriftState {
                agent: AgentId::PI,
                opened_at: now,
            }],
            backup_failed_at: Some(now),
            ..AlertInputs::new(Timestamp::UNIX_EPOCH)
        };
        let first = evaluate_alerts(&inputs, &AlertConfig::default());
        assert_eq!(first.len(), 2);
        inputs.already_fired = first.iter().map(|a| a.dedup_key.clone()).collect();
        assert_eq!(
            evaluate_alerts(&inputs, &AlertConfig::default()),
            Vec::new()
        );
        Ok(())
    }
}
