//! Thin Tauri shell: maps commands to `tokenmtr-query` and `tokenmtr-control`.
//!
//! Layout: [`backend`] holds the plain functions behind every command (testable without a
//! window); [`commands`] wraps them for the webviews; [`push`], [`notify`], [`tray`],
//! [`windows`] and [`shortcut`] are the background and native parts; [`bindings`] renders the
//! command list the UI checks itself against.

pub mod alerts;
pub mod app_store;
pub mod backend;
pub mod bindings;
pub mod commands;
pub mod daemon;
pub mod error;
#[cfg(test)]
mod goldens;
pub mod notify;
pub mod push;
pub mod request;
pub mod shortcut;
pub mod state;
pub mod tray;
pub mod windows;

#[cfg(target_os = "macos")]
use std::time::Instant;

use tauri::{AppHandle, Listener as _, Manager as _, RunEvent, Wry};
use tauri_plugin_autostart::MacosLauncher;
use tokenmtr_platform::ServiceError;

use crate::app_store::AppStore;
use crate::backend::Backend;
use crate::daemon::{RepairOutcome, Sidecar, install_service};
use crate::request::DashboardTarget;
use crate::state::Shell;

/// Points an installed launchd plist at this app's sidecar when the `.app` moved.
#[cfg(target_os = "macos")]
fn check_service(backend: &Backend, sidecar: &Sidecar) -> Result<RepairOutcome, ServiceError> {
    daemon::repair_plist(backend.platform().launchd(), sidecar)
}

/// Whether a systemd user unit exists for the installed package's sidecar; the unit names the
/// same path across upgrades, so there is nothing to repair.
#[cfg(target_os = "linux")]
fn check_service(backend: &Backend, sidecar: &Sidecar) -> Result<RepairOutcome, ServiceError> {
    daemon::check_unit(backend.service(), sidecar)
}

/// Repairs the background service when this app moved (macOS), reinstalls it when it is missing
/// after onboarding, and restarts a daemon that still runs another version than this app (after
/// an update) on the bundled one. A missing service after onboarding is mostly a `brew upgrade`:
/// the cask's `uninstall` removes it, then Homebrew reopens the new app.
/// Runs off the main thread: the service manager may take seconds.
async fn repair_service(backend: Backend, onboarded: bool) {
    let Ok(sidecar) = Sidecar::locate() else {
        return;
    };
    let checked = backend::blocking({
        let (backend, sidecar) = (backend.clone(), sidecar.clone());
        move || Ok(check_service(&backend, &sidecar)?)
    })
    .await;
    let action = match checked {
        Ok(RepairOutcome::NotInstalled) if onboarded => "reinstall the missing daemon",
        Ok(RepairOutcome::Current) if daemon::outdated(backend.transport()).await => {
            "restart the outdated daemon on this app's version"
        }
        Ok(RepairOutcome::Repaired) => {
            tracing::info!("daemon plist repaired after the app moved");
            return;
        }
        Ok(_) => return,
        Err(error) => {
            tracing::warn!(%error, "could not verify the background service");
            return;
        }
    };
    match backend::blocking(move || Ok(install_service(backend.service(), &sidecar)?)).await {
        Ok(()) => tracing::info!(action, "daemon service installed"),
        Err(error) => tracing::warn!(%error, action, "could not install the daemon service"),
    }
}

/// Reacts to what the daemon pushed: title and config-driven registrations on every change,
/// notifications only when an alert fired.
fn react_to_pushes(app: &AppHandle<Wry>) {
    for event in [push::STORE_CHANGED, push::ALERT_FIRED] {
        let handle = app.clone();
        app.listen(event, move |_| {
            tray::refresh(&handle);
            shortcut::sync(&handle);
        });
    }
    let handle = app.clone();
    app.listen(push::ALERT_FIRED, move |_| {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move { notify::deliver_alerts(&handle).await });
    });
}

/// The argument autostart launches the app with: stay in the tray, open no window.
const BACKGROUND_FLAG: &str = "--background";

fn setup(app: &mut tauri::App<Wry>) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let backend = Backend::detect()?;
    let store = AppStore::open(&handle)?;
    let onboarded = store.onboarding_complete();
    let shell = Shell::new(backend.clone(), store);
    let gate = shell.recap_gate.clone();
    app.manage(shell);
    // No Dock icon until the main window opens.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    tauri::async_runtime::spawn(repair_service(backend.clone(), onboarded));
    windows::create_recap(&handle, gate)?;
    tray::install(&handle)?;
    tray::refresh(&handle);
    shortcut::sync(&handle);
    react_to_pushes(&handle);
    tauri::async_runtime::spawn(push::run(backend.transport().clone(), handle.clone()));
    tauri::async_runtime::spawn(notify::watch_stale(handle.clone()));
    // A user launch opens the window: GNOME shows no tray without an extension. Autostart passes
    // [`BACKGROUND_FLAG`] and stays in the tray.
    if !std::env::args().any(|arg| arg == BACKGROUND_FLAG) {
        let target = DashboardTarget {
            route: request::Route::Overview,
            agent: None,
        };
        if let Err(error) = windows::show_main(&handle, &target) {
            tracing::warn!(%error, "could not open the main window at launch");
        }
    }
    Ok(())
}

/// Starts the Tauri application.
///
/// # Panics
///
/// Panics when Tauri cannot start; there is nothing to recover to at that point.
#[expect(
    clippy::panic,
    reason = "startup failure is unrecoverable for a GUI process"
)]
pub fn run() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let target = DashboardTarget {
                route: request::Route::Overview,
                agent: None,
            };
            if let Err(error) = windows::show_main(app, &target) {
                tracing::warn!(%error, "second launch could not open the main window");
            }
        }))
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_FLAG]),
        ))
        .plugin(shortcut::plugin())
        .setup(setup)
        .invoke_handler(commands::handler())
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("error while building TokenMtr: {error}"));
    app.run(|app, event| {
        // Only the macOS Dock reopen needs the handle.
        #[cfg(not(target_os = "macos"))]
        let _ = app;
        match event {
            // The tray keeps the app alive when the last window closes; only an explicit
            // `exit` (the tray's Quit) ends it.
            RunEvent::ExitRequested {
                code: None, api, ..
            } => api.prevent_exit(),
            // Clicking the Dock icon or a notification activates the app: open the main window,
            // at the notification's page when one was just shown.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => {
                let route = app.state::<Shell>().take_route(Instant::now());
                if let Err(error) = windows::show_main(app, &DashboardTarget { route, agent: None })
                {
                    tracing::warn!(%error, "could not open the main window on activation");
                }
            }
            _ => {}
        }
    });
}
