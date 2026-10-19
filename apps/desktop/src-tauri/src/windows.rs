//! The two windows: the recap popover under the tray icon, and the main window that
//! exists only while it is open.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter as _, LogicalSize, Manager as _, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry,
};
use tauri_plugin_positioner::{Position, WindowExt as _};
use tokenmtr_core::brand::DISPLAY_NAME;

use crate::push::NAVIGATE;
use crate::request::DashboardTarget;

/// Label of the recap popover.
pub const RECAP: &str = "recap";
/// Label of the main window.
pub const MAIN: &str = "main";
/// Emitted to the recap webview each time it is shown, so it refreshes then and only then.
pub const RECAP_SHOWN: &str = "recap-shown";

const RECAP_WIDTH: f64 = 360.0;
/// Height before the webview first reports its content, in logical pixels.
const RECAP_HEIGHT: f64 = 520.0;
/// The popover never shrinks below this, so loading and empty states keep a usable size.
const RECAP_MIN_HEIGHT: f64 = 240.0;
/// Gap kept between the popover and the screen edges, in logical pixels.
const MARGIN: f64 = 8.0;
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
/// tray click, or a tray without click events): the top-right corner of `monitor`, the usable
/// area that leaves out the menu bar or panel, kept inside the screen.
#[must_use]
pub fn corner_position(monitor: Area, window: (f64, f64), scale: f64) -> (f64, f64) {
    let (width, height) = window;
    let x = (monitor.x + monitor.width - width - MARGIN * scale).max(monitor.x);
    let y = (monitor.y + MARGIN * scale).min(monitor.y + (monitor.height - height).max(0.0));
    (x, y)
}

/// Pre-creates the hidden recap popover; the first tray click then only shows it.
///
/// # Errors
///
/// The webview cannot be created.
pub fn create_recap(app: &AppHandle<Wry>, gate: Arc<Mutex<RecapGate>>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, RECAP, WebviewUrl::App("tray.html".into()))
        .title(format!("{DISPLAY_NAME} recap"))
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
    let work = monitor.work_area();
    let PhysicalPosition { x, y } = work.position;
    let PhysicalSize { width, height } = work.size;
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

/// The popover height showing `content` logical pixels whole: at least [`RECAP_MIN_HEIGHT`], at
/// most `available` (the screen's usable height less the margins), where the recap scrolls.
/// A non-finite `content` keeps the default height.
#[must_use]
pub fn recap_height(content: f64, available: f64) -> f64 {
    if !content.is_finite() {
        return RECAP_HEIGHT;
    }
    let available = if available.is_finite() {
        available.max(RECAP_MIN_HEIGHT)
    } else {
        RECAP_HEIGHT
    };
    content.clamp(RECAP_MIN_HEIGHT, available)
}

/// Resizes the popover to its content (`content` logical pixels tall), within the screen.
///
/// # Errors
///
/// The window cannot be resized.
pub fn fit_recap(app: &AppHandle<Wry>, content: f64) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(RECAP) else {
        return Ok(());
    };
    let available = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .map_or(RECAP_HEIGHT, |monitor| {
            let work = monitor.work_area().size.height;
            f64::from(work) / monitor.scale_factor() - 2.0 * MARGIN
        });
    window.set_size(LogicalSize::new(
        RECAP_WIDTH,
        recap_height(content, available),
    ))
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

/// Opens the main window at `target`, creating it when it is closed. On macOS, while it exists
/// the app has a Dock icon and a menu bar; when it is destroyed the app goes back to being a
/// tray app.
///
/// # Errors
///
/// The window cannot be created, shown or told to navigate.
pub fn show_main(app: &AppHandle<Wry>, target: &DashboardTarget) -> tauri::Result<()> {
    hide_recap(app);
    if let Some(window) = app.get_webview_window(MAIN) {
        // Only the main window navigates; the recap has no routes.
        app.emit_to(MAIN, NAVIGATE, target)?;
        return focus(app, &window);
    }
    let window = WebviewWindowBuilder::new(
        app,
        MAIN,
        WebviewUrl::App(format!("index.html{}", target.fragment()).into()),
    )
    .title(DISPLAY_NAME)
    .inner_size(1100.0, 760.0)
    .min_inner_size(320.0, 480.0)
    .build()?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event
            && let Err(error) = set_dock_icon(&handle, false)
        {
            tracing::warn!(%error, "could not go back to a tray-only app");
        }
    });
    focus(app, &window)
}

/// macOS shows a Dock icon and a menu bar only while a window is open.
#[cfg(target_os = "macos")]
fn set_dock_icon(app: &AppHandle<Wry>, visible: bool) -> tauri::Result<()> {
    app.set_activation_policy(if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    })
}

/// Other desktops list a window in the taskbar and need no switch.
#[cfg(not(target_os = "macos"))]
#[expect(clippy::unnecessary_wraps, reason = "same signature as the macOS one")]
const fn set_dock_icon(_app: &AppHandle<Wry>, _visible: bool) -> tauri::Result<()> {
    Ok(())
}

fn focus(app: &AppHandle<Wry>, window: &WebviewWindow<Wry>) -> tauri::Result<()> {
    set_dock_icon(app, true)?;
    window
        .show()
        .and_then(|()| window.unminimize())
        .and_then(|()| window.set_focus())
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
    fn recap_fits_its_content_within_the_screen() {
        assert!((recap_height(700.0, 1000.0) - 700.0).abs() < f64::EPSILON);
        assert!((recap_height(1400.0, 1000.0) - 1000.0).abs() < f64::EPSILON);
        assert!((recap_height(100.0, 1000.0) - RECAP_MIN_HEIGHT).abs() < f64::EPSILON);
        assert!((recap_height(f64::NAN, 1000.0) - RECAP_HEIGHT).abs() < f64::EPSILON);
        assert!((recap_height(700.0, 100.0) - RECAP_MIN_HEIGHT).abs() < f64::EPSILON);
    }

    #[test]
    fn corner_hugs_the_top_right_of_the_usable_area() {
        let (x, y) = corner_position(SCREEN, (720.0, 1040.0), 2.0);
        assert!((x - (2880.0 - 720.0 - 16.0)).abs() < f64::EPSILON);
        assert!((y - 16.0).abs() < f64::EPSILON);
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
