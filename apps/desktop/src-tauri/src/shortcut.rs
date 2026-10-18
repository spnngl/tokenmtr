//! The global shortcut (D44): `menubar.shortcut` toggles the recap, re-read when the config
//! may have changed.

use tauri::{AppHandle, Manager as _, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt as _, ShortcutState};

use crate::state::Shell;
use crate::windows;

/// What to do to move from the registered shortcut to the configured one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Registered now, to drop.
    pub unregister: Option<String>,
    /// To register.
    pub register: Option<String>,
}

/// The change from `current` to `wanted`, `None` when they already agree.
#[must_use]
pub fn plan(current: Option<&str>, wanted: Option<&str>) -> Option<Change> {
    let wanted = wanted.filter(|shortcut| !shortcut.trim().is_empty());
    (current != wanted).then(|| Change {
        unregister: current.map(str::to_owned),
        register: wanted.map(str::to_owned),
    })
}

/// The plugin, with the handler that toggles the recap.
#[must_use]
pub fn plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                windows::toggle_recap(app, &app.state::<Shell>().recap_gate);
            }
        })
        .build()
}

/// Registers the configured shortcut, replacing the previous one when the config changed.
pub fn sync(app: &AppHandle<Wry>) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let shell = handle.state::<Shell>().inner().clone();
        let backend = shell.backend.clone();
        let wanted = crate::backend::blocking(move || {
            Ok(backend.context()?.config().menubar.shortcut.clone())
        })
        .await;
        let wanted = match wanted {
            Ok(wanted) => wanted,
            Err(error) => {
                tracing::debug!(%error, "config unreadable; shortcut unchanged");
                return;
            }
        };
        let Ok(mut current) = shell.shortcut.lock() else {
            return;
        };
        let Some(change) = plan(current.as_deref(), wanted.as_deref()) else {
            return;
        };
        let shortcuts = handle.global_shortcut();
        if let Some(old) = &change.unregister
            && let Err(error) = shortcuts.unregister(old.as_str())
        {
            tracing::warn!(%error, shortcut = %old, "could not unregister the shortcut");
        }
        // Remembered even when registration fails, so an invalid shortcut is reported once per
        // config change rather than on every sync.
        (*current).clone_from(&change.register);
        if let Some(new) = change.register
            && let Err(error) = shortcuts.register(new.as_str())
        {
            tracing::warn!(%error, shortcut = %new, "invalid or taken shortcut");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_registers_replaces_and_clears() {
        assert_eq!(plan(None, None), None);
        assert_eq!(plan(Some("Alt+U"), Some("Alt+U")), None);
        assert_eq!(
            plan(None, Some("Alt+U")),
            Some(Change {
                unregister: None,
                register: Some("Alt+U".to_owned())
            })
        );
        assert_eq!(
            plan(Some("Alt+U"), Some("Alt+I")),
            Some(Change {
                unregister: Some("Alt+U".to_owned()),
                register: Some("Alt+I".to_owned())
            })
        );
        assert_eq!(
            plan(Some("Alt+U"), Some(" ")),
            Some(Change {
                unregister: Some("Alt+U".to_owned()),
                register: None
            })
        );
    }
}
