//! The two windows (D29, D47): the recap popover under the tray icon, and the main window that
//! exists only while it is open.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{
    ActivationPolicy, AppHandle, Emitter as _, Manager as _, PhysicalPosition, PhysicalSize,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry,
};
use tauri_plugin_positioner::{Position, WindowExt as _};

use crate::push::NAVIGATE;
use crate::request::DashboardTarget;

/// Label of the recap popover.
pub const RECAP: &str = "recap";
/// Label of the main window.
pub const MAIN: &str = "main";
/// Emitted to the recap webview each time it is shown, so it refreshes then and only then.
pub const RECAP_SHOWN: &str = "recap-shown";

const RECAP_WIDTH: f64 = 360.0;
const RECAP_HEIGHT: f64 = 520.0;
/// How long after a blur-hide a tray click still counts as the click that caused the blur.
const BLUR_GRACE: Duration = Duration::from_millis(250);

/// What a tray click or shortcut press does to the popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// Show it.
    Show,
    /// Hide it.
    Hide,
    /// Do nothing: the same click just hid it by taking the focus away.
    Ignore,
}

/// Decides toggles. Clicking the tray icon while the popover is open first blurs (and hides)
/// the popover, then delivers the click; without this the click would reopen it at once.
#[derive(Debug, Default)]
pub struct RecapGate {
    blurred_at: Option<Instant>,
}

impl RecapGate {
    /// The popover lost focus and was hidden.
    pub fn blurred(&mut self, now: Instant) {
        self.blurred_at = Some(now);
    }

    /// What a toggle request at `now` should do.
    #[must_use]
    pub fn toggle(&self, visible: bool, now: Instant) -> Toggle {
        if visible {
            return Toggle::Hide;
        }
        match self.blurred_at {
            Some(at) if now.saturating_duration_since(at) < BLUR_GRACE => Toggle::Ignore,
            _ => Toggle::Show,
        }
    }
}

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Where the popover goes when the tray icon's position is unknown (a shortcut press before any
/// tray click): the top-right corner of `monitor`, below the menu bar, kept inside the screen.
#[must_use]
pub fn corner_position(monitor: Area, window: (f64, f64), scale: f64) -> (f64, f64) {
    const MENU_BAR: f64 = 28.0;
    const MARGIN: f64 = 8.0;
    let (width, height) = window;
    let x = (monitor.x + monitor.width - width - MARGIN * scale).max(monitor.x);
    let y = (monitor.y + MENU_BAR * scale).min(monitor.y + (monitor.height - height).max(0.0));
    (x, y)
}

/// Pre-creates the hidden recap popover; the first tray click then only shows it.
///
/// # Errors
///
/// The webview cannot be created.
pub fn create_recap(app: &AppHandle<Wry>, gate: Arc<Mutex<RecapGate>>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, RECAP, WebviewUrl::App("tray.html".into()))
        .title("TokenMtr recap")
        .inner_size(RECAP_WIDTH, RECAP_HEIGHT)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .shadow(true)
        .build()?;
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            if let Ok(mut gate) = gate.lock() {
                gate.blurred(Instant::now());
            }
            if let Err(error) = handle.hide() {
                tracing::warn!(%error, "could not hide the recap");
            }
        }
    });
    Ok(())
}

fn place_recap(window: &WebviewWindow<Wry>) {
    if window.move_window(Position::TrayBottomCenter).is_ok() {
        return;
    }
    let Ok(Some(monitor)) = window
        .current_monitor()
        .or_else(|_| window.primary_monitor())
    else {
        return;
    };
    let PhysicalPosition { x, y } = *monitor.position();
    let PhysicalSize { width, height } = *monitor.size();
    let scale = monitor.scale_factor();
    let Ok(size) = window.outer_size() else {
        return;
    };
    let (left, top) = corner_position(
        Area {
            x: f64::from(x),
            y: f64::from(y),
            width: f64::from(width),
            height: f64::from(height),
        },
        (f64::from(size.width), f64::from(size.height)),
        scale,
    );
    if let Err(error) = window.set_position(PhysicalPosition::new(left, top)) {
        tracing::warn!(%error, "could not place the recap");
    }
}

/// Shows, hides or ignores per `gate`.
pub fn toggle_recap(app: &AppHandle<Wry>, gate: &Mutex<RecapGate>) {
    let Some(window) = app.get_webview_window(RECAP) else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let decision = gate
        .lock()
        .map_or(Toggle::Show, |gate| gate.toggle(visible, Instant::now()));
    match decision {
        Toggle::Ignore => {}
        Toggle::Hide => {
            if let Err(error) = window.hide() {
                tracing::warn!(%error, "could not hide the recap");
            }
        }
        Toggle::Show => {
            place_recap(&window);
            if let Err(error) = window.show().and_then(|()| window.set_focus()) {
                tracing::warn!(%error, "could not show the recap");
            }
            if let Err(error) = app.emit_to(RECAP, RECAP_SHOWN, ()) {
                tracing::warn!(%error, "could not tell the recap it is shown");
            }
        }
    }
}

/// Hides the popover (the user opened the main window from it).
pub fn hide_recap(app: &AppHandle<Wry>) {
    if let Some(window) = app.get_webview_window(RECAP)
        && let Err(error) = window.hide()
    {
        tracing::warn!(%error, "could not hide the recap");
    }
}

/// Opens the main window at `target`, creating it when it is closed. While it exists the app
/// has a Dock icon and a menu bar; when it is destroyed the app goes back to being a tray app.
///
/// # Errors
///
/// The target is invalid, or the window cannot be created.
pub fn show_main(app: &AppHandle<Wry>, target: &DashboardTarget) -> Result<(), String> {
    let fragment = target.fragment()?;
    hide_recap(app);
    if let Some(window) = app.get_webview_window(MAIN) {
        app.emit(
            NAVIGATE,
            serde_json::json!({ "route": target.route, "agent": target.agent }),
        )
        .map_err(|error| error.to_string())?;
        return focus(app, &window);
    }
    let window = WebviewWindowBuilder::new(
        app,
        MAIN,
        WebviewUrl::App(format!("index.html{fragment}").into()),
    )
    .title("TokenMtr")
    .inner_size(1100.0, 760.0)
    .min_inner_size(320.0, 480.0)
    .build()
    .map_err(|error| error.to_string())?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event
            && let Err(error) = handle.set_activation_policy(ActivationPolicy::Accessory)
        {
            tracing::warn!(%error, "could not go back to a tray-only app");
        }
    });
    focus(app, &window)
}

fn focus(app: &AppHandle<Wry>, window: &WebviewWindow<Wry>) -> Result<(), String> {
    app.set_activation_policy(ActivationPolicy::Regular)
        .map_err(|error| error.to_string())?;
    window
        .show()
        .and_then(|()| window.unminimize())
        .and_then(|()| window.set_focus())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = Area {
        x: 0.0,
        y: 0.0,
        width: 2880.0,
        height: 1800.0,
    };

    #[test]
    fn a_click_right_after_a_blur_is_the_one_that_closed_it() {
        let start = Instant::now();
        let mut gate = RecapGate::default();
        assert_eq!(gate.toggle(false, start), Toggle::Show);
        assert_eq!(gate.toggle(true, start), Toggle::Hide);
        gate.blurred(start);
        assert_eq!(
            gate.toggle(false, start + Duration::from_millis(50)),
            Toggle::Ignore
        );
        assert_eq!(
            gate.toggle(false, start + Duration::from_secs(2)),
            Toggle::Show
        );
    }

    #[test]
    fn corner_hugs_the_top_right_below_the_menu_bar() {
        let (x, y) = corner_position(SCREEN, (720.0, 1040.0), 2.0);
        assert!((x - (2880.0 - 720.0 - 16.0)).abs() < f64::EPSILON);
        assert!((y - 56.0).abs() < f64::EPSILON);
    }

    #[test]
    fn corner_stays_on_a_small_or_offset_screen() {
        let small = Area {
            x: -1440.0,
            y: 0.0,
            width: 600.0,
            height: 500.0,
        };
        let (x, y) = corner_position(small, (720.0, 1040.0), 2.0);
        assert!((x - -1440.0).abs() < f64::EPSILON);
        assert!(y.abs() < f64::EPSILON);
    }
}
