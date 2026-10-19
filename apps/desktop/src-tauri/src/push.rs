//! Push: one control-socket `Subscribe` task re-emitted to the webviews.
//!
//! The daemon announces what changed; nothing in the app polls. When the connection drops the
//! task reconnects with a capped backoff and tells the UI through `daemon-state`.

use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Runtime};
use tokenmtr_control::{Client, Event};
use tokenmtr_platform::ControlTransport;

use crate::daemon::{Link, client_info};

/// The store changed; payload `{ "datasets": [...] }` (empty after a reconnect: refresh all).
pub const STORE_CHANGED: &str = "store-changed";
/// An alert fired; payload is the alert (`kind`, `dedup_key`, `at`).
pub const ALERT_FIRED: &str = "alert-fired";
/// The daemon link changed; payload is a [`Link`].
pub const DAEMON_STATE: &str = "daemon-state";
/// The main window should go to a route; payload `{ "route": "/quota", "agent": null }`.
pub const NAVIGATE: &str = "navigate";

/// Where events go. The webviews in production, a recorder in tests.
pub trait Emit: Send + Sync + 'static {
    /// Broadcasts `payload` as `event`.
    fn emit(&self, event: &str, payload: Value);
}

impl<R: Runtime> Emit for AppHandle<R> {
    fn emit(&self, event: &str, payload: Value) {
        if let Err(error) = Emitter::emit(self, event, payload) {
            tracing::warn!(%error, event, "could not emit to the webviews");
        }
    }
}

/// The first retry delay.
const BACKOFF_START: Duration = Duration::from_secs(1);
/// The longest wait between two attempts.
const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// How long a subscription must last before the backoff starts over, so a daemon that keeps
/// restarting is not reconnected every second.
const STABLE: Duration = Duration::from_secs(10);

/// Doubling delay between reconnect attempts, from one second up to thirty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            next: BACKOFF_START,
        }
    }
}

impl Backoff {
    /// The delay to wait now; the following one is doubled.
    pub fn next_delay(&mut self) -> Duration {
        let now = self.next;
        self.next = (self.next * 2).min(BACKOFF_CAP);
        now
    }

    /// Back to the first delay, after a connection that worked.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Remembers the last link and reports only changes, so the UI hears each transition once.
#[derive(Debug, Default)]
pub struct LinkTracker {
    last: Option<Link>,
}

impl LinkTracker {
    /// Emits `daemon-state` when `link` differs from the previous one.
    pub fn report(&mut self, link: Link, sink: &dyn Emit) {
        if self.last.as_ref() == Some(&link) {
            return;
        }
        match serde_json::to_value(&link) {
            Ok(payload) => sink.emit(DAEMON_STATE, payload),
            Err(error) => tracing::error!(%error, "daemon link is not serialisable"),
        }
        self.last = Some(link);
    }
}

/// The webview event a daemon event becomes: its name and payload.
#[must_use]
pub fn translate(event: &Event) -> Option<(&'static str, Value)> {
    match event {
        Event::StoreChanged { datasets } => serde_json::to_value(datasets)
            .ok()
            .map(|datasets| (STORE_CHANGED, json!({ "datasets": datasets }))),
        Event::AlertFired { alert } => serde_json::to_value(alert)
            .ok()
            .map(|alert| (ALERT_FIRED, alert)),
    }
}

/// One connection's life. Returns whether the subscription stayed up for [`STABLE`] (to reset
/// the backoff).
async fn serve_once(
    transport: &ControlTransport,
    tracker: &mut LinkTracker,
    sink: &dyn Emit,
) -> bool {
    let client = match Client::connect(transport, client_info()).await {
        Ok(client) => client,
        Err(error) => {
            tracker.report(Link::from_error(&error), sink);
            return false;
        }
    };
    let connected = Link::from_welcome(client.daemon());
    if matches!(connected, Link::Incompatible { .. }) {
        // Another store schema: this app cannot read what the daemon announces.
        tracker.report(connected, sink);
        return false;
    }
    let since = tokio::time::Instant::now();
    let mut subscription = match client.subscribe().await {
        Ok(subscription) => subscription,
        Err(error) => {
            tracing::warn!(%error, "daemon refused the subscription");
            tracker.report(Link::Offline, sink);
            return false;
        }
    };
    tracker.report(connected, sink);
    // Anything may have changed while nobody listened.
    sink.emit(STORE_CHANGED, json!({ "datasets": [] }));
    loop {
        match subscription.next().await {
            Ok(Some(event)) => {
                if let Some((name, payload)) = translate(&event) {
                    sink.emit(name, payload);
                }
            }
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "daemon connection lost");
                break;
            }
        }
    }
    tracker.report(Link::Offline, sink);
    since.elapsed() >= STABLE
}

/// Subscribes forever, reconnecting with backoff.
pub async fn run(transport: ControlTransport, sink: impl Emit) {
    let mut tracker = LinkTracker::default();
    let mut backoff = Backoff::default();
    loop {
        if serve_once(&transport, &mut tracker, &sink).await {
            backoff.reset();
        }
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, Value)>>);

    impl Emit for &'static Recorder {
        fn emit(&self, event: &str, payload: Value) {
            if let Ok(mut log) = self.0.lock() {
                log.push((event.to_owned(), payload));
            }
        }
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_resets() {
        let mut backoff = Backoff::default();
        let delays: Vec<u64> = (0..7).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn tracker_reports_transitions_once() {
        let log: &'static Recorder = Box::leak(Box::default());
        let mut tracker = LinkTracker::default();
        tracker.report(Link::Offline, &log);
        tracker.report(Link::Offline, &log);
        tracker.report(
            Link::Connected {
                daemon_version: "1".to_owned(),
            },
            &log,
        );
        tracker.report(
            Link::Incompatible {
                daemon_version: "9".to_owned(),
                daemon_schema: 1,
            },
            &log,
        );
        let states: Vec<String> = log
            .0
            .lock()
            .map(|log| {
                log.iter()
                    .map(|(name, payload)| format!("{name}:{}", payload["state"]))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            states,
            [
                "daemon-state:\"offline\"",
                "daemon-state:\"connected\"",
                "daemon-state:\"incompatible\""
            ]
        );
    }

    #[test]
    fn store_changes_become_webview_events() {
        let event = Event::StoreChanged {
            datasets: Vec::new(),
        };
        assert_eq!(
            translate(&event),
            Some((STORE_CHANGED, json!({ "datasets": [] })))
        );
    }

    #[tokio::test]
    async fn a_missing_daemon_reports_offline_once() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir_in("/tmp")?;
        let transport = ControlTransport::unix(
            camino::Utf8PathBuf::from_path_buf(dir.path().join("control.sock"))
                .map_err(|path| format!("{path:?}"))?,
        );
        let log: &'static Recorder = Box::leak(Box::default());
        let mut tracker = LinkTracker::default();
        assert!(!serve_once(&transport, &mut tracker, &log).await);
        assert!(!serve_once(&transport, &mut tracker, &log).await);
        let count = log.0.lock().map(|log| log.len()).unwrap_or_default();
        assert_eq!(count, 1);
        Ok(())
    }
}
