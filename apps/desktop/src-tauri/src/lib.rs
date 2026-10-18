//! Thin Tauri shell: maps commands to `tokenmtr-query` and `tokenmtr-control` (D2).
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
pub mod notify;
pub mod push;
pub mod request;
pub mod shortcut;
pub mod state;
pub mod tray;
pub mod windows;

use std::time::Instant;

use tauri::{AppHandle, Listener as _, Manager as _, RunEvent, Wry};
use tauri_plugin_autostart::MacosLauncher;
use tokenmtr_platform::{LaunchdService, SystemRunner};

use crate::app_store::AppStore;
use crate::backend::Backend;
use crate::daemon::{RepairOutcome, Sidecar, repair_plist};
use crate::request::DashboardTarget;
use crate::state::Shell;

/// Points an installed plist at this app's sidecar when the `.app` moved (D23).
fn repair_service(backend: &Backend) {
    let Ok(sidecar) = Sidecar::locate() else {
        return;
    };
    let service = LaunchdService::new(backend.paths(), SystemRunner);
    match repair_plist(&service, &sidecar) {
        Ok(RepairOutcome::Repaired) => tracing::info!("daemon plist repaired after the app moved"),
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "could not verify the daemon plist"),
    }
}

/// Reacts to what the daemon pushed: title, notifications, config-driven registrations.
fn react_to_pushes(app: &AppHandle<Wry>) {
    for event in [push::STORE_CHANGED, push::ALERT_FIRED] {
        let handle = app.clone();
        app.listen(event, move |_| {
            tray::refresh(&handle);
            shortcut::sync(&handle);
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move { notify::deliver_alerts(&handle).await });
        });
    }
}

fn setup(app: &mut tauri::App<Wry>) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let backend = Backend::detect()?;
    let store = AppStore::open(&handle)?;
    let shell = Shell::new(backend.clone(), store);
    let gate = shell.recap_gate.clone();
    app.manage(shell);
    // A tray-only app: no Dock icon until the main window opens (D29).
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    repair_service(&backend);
    windows::create_recap(&handle, gate)?;
    tray::install(&handle)?;
    tray::refresh(&handle);
    shortcut::sync(&handle);
    react_to_pushes(&handle);
    tauri::async_runtime::spawn(push::run(backend.transport().clone(), handle.clone()));
    tauri::async_runtime::spawn(notify::watch_stale(handle));
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
            None,
        ))
        .plugin(shortcut::plugin())
        .setup(setup)
        .invoke_handler(commands::handler())
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("error while building TokenMtr: {error}"));
    app.run(|app, event| match event {
        // The tray keeps the app alive when the last window closes; only an explicit
        // `exit` (the tray's Quit) ends it.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        // Clicking the Dock icon or a notification activates the app: open the main window,
        // at the notification's page when one was just shown.
        RunEvent::Reopen { .. } => {
            let route = app.state::<Shell>().take_route(Instant::now());
            if let Err(error) = windows::show_main(app, &DashboardTarget { route, agent: None }) {
                tracing::warn!(%error, "could not open the main window on activation");
            }
        }
        _ => {}
    });
}
