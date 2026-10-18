//! What the app itself remembers between launches, in `app-state.json`.
//!
//! Three typed values behind one small wrapper, so the rest of the shell never touches raw keys.

use std::sync::Arc;

use tauri::{AppHandle, Runtime};
use tauri_plugin_store::{Store, StoreExt as _};

use crate::alerts::Delivered;
use crate::error::CommandError;

/// File name inside the app's data directory.
const FILE: &str = "app-state.json";
const KEY_DELIVERED: &str = "delivered_alerts";
const KEY_STALE: &str = "stale_announced_heartbeat_us";
const KEY_ONBOARDING: &str = "onboarding_complete";

/// The persisted app state.
pub struct AppStore<R: Runtime>(Arc<Store<R>>);

impl<R: Runtime> std::fmt::Debug for AppStore<R> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AppStore").finish_non_exhaustive()
    }
}

impl<R: Runtime> Clone for AppStore<R> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<R: Runtime> AppStore<R> {
    /// Opens (creating when missing) the state file.
    ///
    /// # Errors
    ///
    /// The file exists but cannot be read or parsed.
    pub fn open(app: &AppHandle<R>) -> Result<Self, CommandError> {
        Ok(Self(app.store(FILE).map_err(CommandError::internal)?))
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<(), CommandError> {
        self.0.set(key, value);
        self.0.save().map_err(CommandError::internal)
    }

    /// The user finished the onboarding.
    #[must_use]
    pub fn onboarding_complete(&self) -> bool {
        self.0
            .get(KEY_ONBOARDING)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    /// Records that onboarding is done (or must be redone after an erase).
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn set_onboarding_complete(&self, done: bool) -> Result<(), CommandError> {
        self.put(KEY_ONBOARDING, done.into())
    }

    /// The alert watermark, absent before the first delivery pass.
    #[must_use]
    pub fn delivered(&self) -> Option<Delivered> {
        self.0
            .get(KEY_DELIVERED)
            .and_then(|value| serde_json::from_value(value).ok())
    }

    /// Stores the alert watermark.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn set_delivered(&self, delivered: &Delivered) -> Result<(), CommandError> {
        let value = serde_json::to_value(delivered).map_err(CommandError::internal)?;
        self.put(KEY_DELIVERED, value)
    }

    /// The heartbeat (µs) of the silence already announced.
    #[must_use]
    pub fn stale_announced(&self) -> Option<i64> {
        self.0.get(KEY_STALE).and_then(|value| value.as_i64())
    }

    /// Remembers the announced silence.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn set_stale_announced(&self, heartbeat_us: i64) -> Result<(), CommandError> {
        self.put(KEY_STALE, heartbeat_us.into())
    }
}
