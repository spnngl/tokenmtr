//! The state every command and background task shares, managed by Tauri.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::Wry;

use crate::app_store::AppStore;
use crate::backend::Backend;
use crate::request::Route;
use crate::windows::RecapGate;

/// How long after a notification the next app activation still counts as a click on it.
const CLICK_WINDOW: Duration = Duration::from_secs(60);

/// The page a just-shown notification leads to.
///
/// The notification plugin has no click callback on macOS, so the shell remembers the route and
/// uses it when the app is activated right afterwards (`RunEvent::Reopen`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    route: Route,
    at: Instant,
}

impl Pending {
    /// A notification for `route` was shown at `at`.
    #[must_use]
    pub const fn new(route: Route, at: Instant) -> Self {
        Self { route, at }
    }

    /// The route when `now` is still within the click window.
    #[must_use]
    pub fn fresh(self, now: Instant) -> Option<Route> {
        (now.saturating_duration_since(self.at) < CLICK_WINDOW).then_some(self.route)
    }
}

/// Everything shared between commands, tray, windows and background tasks.
#[derive(Debug, Clone)]
pub struct Shell {
    /// Where the config, store and socket are.
    pub backend: Backend,
    /// What the app remembers between launches.
    pub store: AppStore<Wry>,
    /// Click-after-blur bookkeeping of the recap popover.
    pub recap_gate: Arc<Mutex<RecapGate>>,
    /// The latest shown notification.
    pub pending: Arc<Mutex<Option<Pending>>>,
    /// The shortcut currently registered.
    pub shortcut: Arc<Mutex<Option<String>>>,
    /// Serialises alert delivery passes.
    pub delivery: Arc<tokio::sync::Mutex<()>>,
}

impl Shell {
    /// A shell over `backend` and `store`.
    #[must_use]
    pub fn new(backend: Backend, store: AppStore<Wry>) -> Self {
        Self {
            backend,
            store,
            recap_gate: Arc::default(),
            pending: Arc::default(),
            shortcut: Arc::default(),
            delivery: Arc::default(),
        }
    }

    /// The route to open on activation: the pending notification's when fresh, else the overview.
    #[must_use]
    pub fn take_route(&self, now: Instant) -> Route {
        self.pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.take())
            .and_then(|pending| pending.fresh(now))
            .unwrap_or(Route::Overview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_route_expires() {
        let start = Instant::now();
        let pending = Pending::new(Route::Quota, start);
        assert_eq!(
            pending.fresh(start + Duration::from_secs(5)),
            Some(Route::Quota)
        );
        assert_eq!(pending.fresh(start + Duration::from_secs(61)), None);
    }
}
