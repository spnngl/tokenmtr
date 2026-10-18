//! Delivers notifications (D36): stored alerts newer than the watermark, and the app-side
//! `daemon_stale` alert.

use std::time::{Duration, Instant};

use jiff::Timestamp;
use tauri::{AppHandle, Manager as _, Wry};
use tauri_plugin_notification::NotificationExt as _;
use tokenmtr_app::AppContext;

use crate::alerts::{BATCH, Delivered, Notice, select_undelivered, stale_decision};
use crate::daemon::{Link, probe};
use crate::error::CommandError;
use crate::state::{Pending, Shell};

/// The shortest pause between two stale checks.
const MIN_STALE_CHECK: Duration = Duration::from_secs(30);

/// New alerts since `mark`, and the watermark after them. Without a watermark (first launch)
/// everything already stored counts as seen: the user is not greeted with history.
///
/// # Errors
///
/// Store failures.
pub fn collect(
    app: &AppContext,
    mark: Option<Delivered>,
    now: Timestamp,
) -> Result<(Vec<Notice>, Option<Delivered>), CommandError> {
    let Some(mark) = mark else {
        return Ok((Vec::new(), Some(Delivered::since(now.as_microsecond()))));
    };
    let since = Timestamp::from_microsecond(mark.at_us).map_err(CommandError::internal)?;
    let rows = app.alerts(since, BATCH)?;
    let (fresh, next) = select_undelivered(&rows, &mark);
    let notices = fresh
        .iter()
        .map(|row| Notice::from_kind(&row.alert))
        .collect();
    let advanced = (next != mark).then_some(next);
    Ok((notices, advanced))
}

fn show(app: &AppHandle<Wry>, shell: &Shell, notice: &Notice) {
    let shown = app
        .notification()
        .builder()
        .title(&notice.title)
        .body(&notice.body)
        .show();
    match shown {
        Ok(()) => {
            if let Ok(mut pending) = shell.pending.lock() {
                *pending = Some(Pending::new(notice.route, Instant::now()));
            }
        }
        Err(error) => tracing::warn!(%error, "could not show a notification"),
    }
}

/// One delivery pass: reads new alerts, notifies, then saves the watermark.
pub async fn deliver_alerts(app: &AppHandle<Wry>) {
    let shell = app.state::<Shell>().inner().clone();
    let _turn = shell.delivery.lock().await;
    let backend = shell.backend.clone();
    let mark = shell.store.delivered();
    let result =
        crate::backend::blocking(move || collect(&backend.context()?, mark, Timestamp::now()))
            .await;
    let (notices, advanced) = match result {
        Ok(found) => found,
        Err(error) => {
            tracing::debug!(%error, "alerts unavailable");
            return;
        }
    };
    for notice in &notices {
        show(app, &shell, notice);
    }
    if let Some(next) = advanced
        && let Err(error) = shell.store.set_delivered(&next)
    {
        tracing::warn!(%error, "could not save the alert watermark");
    }
}

/// The heartbeat to announce, and how long to wait before checking again.
///
/// # Errors
///
/// Config and store failures.
pub fn stale_check(
    app: &AppContext,
    announced: Option<i64>,
    now: Timestamp,
) -> Result<(Option<i64>, Duration), CommandError> {
    let stale_after = app.config().alerts.daemon_stale_after;
    let ctx = app.query_context(now)?;
    let health = tokenmtr_query::daemon_health(app.reader(), &ctx, stale_after)?;
    let half = Duration::try_from(stale_after / 2).unwrap_or(MIN_STALE_CHECK);
    Ok((
        stale_decision(&health, announced),
        half.max(MIN_STALE_CHECK),
    ))
}

/// Watches the daemon heartbeat for the app's lifetime and raises `daemon_stale` once per silence.
pub async fn watch_stale(app: AppHandle<Wry>) {
    loop {
        let shell = app.state::<Shell>().inner().clone();
        let backend = shell.backend.clone();
        let announced = shell.store.stale_announced();
        let checked = crate::backend::blocking(move || {
            stale_check(&backend.context()?, announced, Timestamp::now())
        })
        .await;
        let wait = match checked {
            // After a sleep the heartbeat is old but the daemon is fine: it beats again soon.
            Ok((Some(_), wait))
                if matches!(
                    probe(shell.backend.transport()).await,
                    Link::Connected { .. }
                ) =>
            {
                wait
            }
            Ok((Some(heartbeat), wait)) => {
                show(&app, &shell, &Notice::daemon_stale());
                if let Err(error) = shell.store.set_stale_announced(heartbeat) {
                    tracing::warn!(%error, "could not remember the stale announcement");
                }
                wait
            }
            Ok((None, wait)) => wait,
            Err(error) => {
                tracing::debug!(%error, "stale check unavailable");
                MIN_STALE_CHECK
            }
        };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fixture::{CONFIG, seeded};

    #[test]
    fn first_launch_skips_history_and_sets_the_watermark() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let (notices, mark) = collect(&app, None, fixture.now)?;
        assert_eq!(notices.len(), 0);
        assert_eq!(mark, Some(Delivered::since(fixture.now.as_microsecond())));
        let (again, unchanged) = collect(&app, mark, fixture.now)?;
        assert_eq!(again.len(), 0);
        assert_eq!(unchanged, None);
        Ok(())
    }

    #[test]
    fn a_daemon_that_never_ran_is_not_stale() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let (announce, wait) = stale_check(&app, None, fixture.now)?;
        assert_eq!(announce, None);
        assert_eq!(wait, Duration::from_secs(450));
        Ok(())
    }
}
