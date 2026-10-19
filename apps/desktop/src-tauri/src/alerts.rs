//! Alert delivery: which alerts are new, what to say, where a click leads.
//!
//! Pure over its inputs; the shell feeds it rows from the store and persists the watermark in
//! `app-state.json` (see `app_store`), so a restart never delivers an alert twice.

use serde::{Deserialize, Serialize};
use tokenmtr_core::AlertKind;
#[cfg(test)]
use tokenmtr_core::WindowId;
use tokenmtr_core::brand::DISPLAY_NAME;
use tokenmtr_query::{AlertView, DaemonHealth, DaemonState};

use crate::request::Route;

/// How many alerts one delivery pass reads; older unseen ones are dropped, not queued.
pub const BATCH: u32 = 20;

/// What was delivered. Alert rows have no id, and `fired_at` follows the wall clock, which may
/// move backwards; so the mark is a fixed floor plus the identities of the newest delivered
/// alerts, not a moving instant that would skip alerts fired after a clock change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivered {
    /// Microseconds since the epoch: alerts fired at or before it are never delivered (the
    /// history at first launch). Marks written before this scheme hold their newest alert here.
    pub at_us: i64,
    /// [`identity`] of every alert of the last batch (at most [`BATCH`]), newest first.
    pub keys: Vec<String>,
}

impl Delivered {
    /// A watermark that treats everything up to `at_us` as delivered (first launch).
    #[must_use]
    pub const fn since(at_us: i64) -> Self {
        Self {
            at_us,
            keys: Vec::new(),
        }
    }
}

/// One alert row: its instant and dedup key.
fn identity(alert: &AlertView) -> String {
    format!("{}:{}", alert.fired_at.as_microsecond(), alert.dedup_key)
}

/// The alerts of `rows` (the newest [`BATCH`] after the floor, newest first) that `mark` has not
/// seen, oldest first, and the mark after delivering them.
#[must_use]
pub fn select_undelivered<'a>(
    rows: &'a [AlertView],
    mark: &Delivered,
) -> (Vec<&'a AlertView>, Delivered) {
    let batch: Vec<&AlertView> = rows
        .iter()
        .filter(|row| row.fired_at.as_microsecond() > mark.at_us)
        .collect();
    let keys: Vec<String> = batch.iter().map(|row| identity(row)).collect();
    let mut fresh: Vec<&AlertView> = batch
        .into_iter()
        .filter(|row| !mark.keys.contains(&identity(row)))
        .collect();
    fresh.reverse();
    let next = Delivered {
        at_us: mark.at_us,
        keys,
    };
    (fresh, next)
}

/// What the notification says and where a click goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Notification title.
    pub title: String,
    /// Notification body.
    pub body: String,
    /// The page that explains it.
    pub route: Route,
}

impl Notice {
    /// The notice for a stored alert.
    #[must_use]
    pub fn from_kind(kind: &AlertKind) -> Self {
        let (title, body, route) = match kind {
            AlertKind::QuotaThreshold { window, percent } => (
                format!("Quota at {percent}%"),
                format!(
                    "The {} window passed {percent}% of its limit.",
                    window.label()
                ),
                Route::Quota,
            ),
            AlertKind::QuotaReset { window } => (
                "Quota reset".to_owned(),
                format!("The {} window has reset.", window.label()),
                Route::Quota,
            ),
            AlertKind::BudgetThreshold { budget, percent } => (
                format!("Budget at {percent}%"),
                format!("Budget {budget} passed {percent}%."),
                Route::Billing,
            ),
            AlertKind::SinkFailing { sink, .. } => (
                "Export failing".to_owned(),
                format!("Sink {sink} has been failing for a while."),
                Route::Settings,
            ),
            AlertKind::FormatDrift { agent } => (
                "Log format changed".to_owned(),
                format!("{agent} wrote lines {DISPLAY_NAME} does not recognise."),
                Route::Settings,
            ),
            AlertKind::BackupFailed => (
                "Backup failed".to_owned(),
                "The last backup did not complete.".to_owned(),
                Route::Settings,
            ),
        };
        Self { title, body, route }
    }

    /// The app-side notice for a daemon whose heartbeat stopped.
    #[must_use]
    pub fn daemon_stale() -> Self {
        Self {
            title: format!("{DISPLAY_NAME} stopped collecting"),
            body: "The background service has not reported for a while.".to_owned(),
            route: Route::Settings,
        }
    }
}

/// The heartbeat (µs) to announce as stale and remember, or `None` when there is nothing to say:
/// the daemon is fine, never ran, or this very silence was announced already.
#[must_use]
pub fn stale_decision(health: &DaemonHealth, announced_for: Option<i64>) -> Option<i64> {
    if health.state != DaemonState::Stale {
        return None;
    }
    let heartbeat = health.last_heartbeat?.as_microsecond();
    (announced_for != Some(heartbeat)).then_some(heartbeat)
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;

    fn row(secs: i64, key: &str) -> Result<AlertView, jiff::Error> {
        Ok(AlertView {
            fired_at: Timestamp::from_second(secs)?,
            dedup_key: key.to_owned(),
            alert: AlertKind::BackupFailed,
        })
    }

    fn keys<'a>(alerts: &[&'a AlertView]) -> Vec<&'a str> {
        alerts.iter().map(|a| a.dedup_key.as_str()).collect()
    }

    #[test]
    fn delivers_alerts_after_the_floor_oldest_first_once() -> Result<(), jiff::Error> {
        let rows = [row(30, "c")?, row(20, "b")?, row(10, "a")?];
        let (fresh, next) = select_undelivered(&rows, &Delivered::since(10_000_000));
        assert_eq!(keys(&fresh), ["b", "c"]);
        assert_eq!(next.at_us, 10_000_000);
        assert_eq!(next.keys, ["30000000:c", "20000000:b"]);
        let (again, same) = select_undelivered(&rows, &next);
        assert_eq!(again.len(), 0);
        assert_eq!(same, next);
        Ok(())
    }

    #[test]
    fn same_instant_is_deduplicated_by_key() -> Result<(), jiff::Error> {
        let mark = Delivered {
            at_us: 0,
            keys: vec!["20000000:x".to_owned()],
        };
        let rows = [row(20, "y")?, row(20, "x")?];
        let (fresh, _) = select_undelivered(&rows, &mark);
        assert_eq!(keys(&fresh), ["y"]);
        Ok(())
    }

    #[test]
    fn an_alert_fired_after_the_clock_went_back_is_delivered() -> Result<(), jiff::Error> {
        let (_, mark) = select_undelivered(&[row(30, "c")?], &Delivered::since(0));
        // The clock moved back: the next alert is stamped before the last delivered one.
        let rows = [row(30, "c")?, row(25, "d")?];
        let (fresh, _) = select_undelivered(&rows, &mark);
        assert_eq!(keys(&fresh), ["d"]);
        Ok(())
    }

    #[test]
    fn routes_follow_the_kind() {
        let quota = AlertKind::QuotaThreshold {
            window: WindowId::five_hour(),
            percent: 80,
        };
        assert_eq!(Notice::from_kind(&quota).route, Route::Quota);
        assert_eq!(
            Notice::from_kind(&AlertKind::BackupFailed).route,
            Route::Settings
        );
        assert_eq!(Notice::daemon_stale().route, Route::Settings);
    }

    fn health(state: DaemonState, beat: Option<i64>) -> Result<DaemonHealth, jiff::Error> {
        Ok(DaemonHealth {
            state,
            last_heartbeat: beat.map(Timestamp::from_second).transpose()?,
            age_secs: None,
        })
    }

    #[test]
    fn stale_is_announced_once_per_silence() -> Result<(), jiff::Error> {
        let stale = health(DaemonState::Stale, Some(100))?;
        assert_eq!(stale_decision(&stale, None), Some(100_000_000));
        assert_eq!(stale_decision(&stale, Some(100_000_000)), None);
        let later = health(DaemonState::Stale, Some(900))?;
        assert_eq!(stale_decision(&later, Some(100_000_000)), Some(900_000_000));
        assert_eq!(
            stale_decision(&health(DaemonState::Running, Some(100))?, None),
            None
        );
        assert_eq!(
            stale_decision(&health(DaemonState::NeverStarted, None)?, None),
            None
        );
        Ok(())
    }
}
