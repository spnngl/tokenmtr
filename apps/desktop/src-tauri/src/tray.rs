//! The menu-bar icon: quota rings when there is something to show, the plain template icon
//! otherwise, and the tray menu.

mod rings;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use jiff::Timestamp;
use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager as _, Wry};
use tokenmtr_config::{MenubarConfig, MenubarRing};
use tokenmtr_core::WindowId;
use tokenmtr_core::brand::{CLI_NAME, DISPLAY_NAME};
use tokenmtr_query::{DateRange, Filters, QuotaMeter};
use tokenmtr_store::QuotaStatus;

pub use self::rings::{Level, Ring};
use crate::backend::actions;
use crate::error::CommandError;
use crate::request::{DashboardTarget, Route};
use crate::state::Shell;
use crate::windows;

const TRAY_ID: &str = CLI_NAME;
/// Monochrome template image (44x44, black on transparent; `tray-icon.py`): macOS tints it.
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-icon@2x.png");
const OPEN: &str = "open";
const SCAN: &str = "scan";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

/// Whether a ring kind covers `window`.
fn covers(kind: MenubarRing, window: &WindowId) -> bool {
    match kind {
        MenubarRing::Session => window.as_str() == WindowId::FIVE_HOUR,
        MenubarRing::Weekly => window.as_str() == WindowId::SEVEN_DAY,
        MenubarRing::PerModel => window.scoped_model().is_some(),
    }
}

/// The rings of each shown profile: the followed agents' profiles that are not hidden, one
/// ring per selected window (session, weekly, then per model) or one dashed ring when the
/// profile's quota cannot be read. Empty when no ring is selected or nothing was polled.
#[must_use]
pub fn ring_groups(meters: &[QuotaMeter], menubar: &MenubarConfig) -> Vec<Vec<Ring>> {
    if menubar.rings.is_empty() {
        return Vec::new();
    }
    meters
        .iter()
        .filter(|meter| {
            menubar.agents.contains(&meter.agent)
                && !menubar.hidden_profiles.contains(&meter.profile)
        })
        .map(|meter| {
            if meter.status != QuotaStatus::Ok {
                return vec![Ring::Unknown];
            }
            MenubarRing::ALL
                .into_iter()
                .filter(|kind| menubar.rings.contains(kind))
                .flat_map(|kind| {
                    meter
                        .windows
                        .iter()
                        .filter(move |window| covers(kind, &window.window))
                })
                .map(|window| Ring::used(window.used_percent))
                .collect()
        })
        .filter(|group: &Vec<Ring>| !group.is_empty())
        .collect()
}

/// The rings for the current store and config.
///
/// # Errors
///
/// Config, store and query failures.
pub fn current_rings(shell: &Shell, now: Timestamp) -> Result<Vec<Vec<Ring>>, CommandError> {
    let app = shell.backend.context()?;
    let ctx = app.query_context(now)?;
    let meters = tokenmtr_query::quota_latest(
        app.reader(),
        &ctx,
        &Filters::new(DateRange::day(ctx.today())?),
    )?;
    Ok(ring_groups(&meters, &app.config().menubar))
}

/// Numbers refreshes, so a slow one never overwrites the picture of a later one.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The rings on screen, so an unchanged picture is not set again.
static SHOWN: Mutex<Option<Vec<Vec<Ring>>>> = Mutex::new(None);

/// Recomputes the rings off the main thread and redraws the icon when they changed.
pub fn refresh(app: &AppHandle<Wry>) {
    let handle = app.clone();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn(async move {
        let shell = handle.state::<Shell>().inner().clone();
        let groups =
            crate::backend::blocking(move || current_rings(&shell, Timestamp::now())).await;
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        let Some(tray) = handle.tray_by_id(TRAY_ID) else {
            return;
        };
        let groups = match groups {
            Ok(groups) => groups,
            Err(error) => {
                tracing::debug!(%error, "tray rings unavailable");
                return;
            }
        };
        let mut shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
        if shown.as_ref() == Some(&groups) {
            return;
        }
        match show(&tray, &groups) {
            Ok(()) => *shown = Some(groups),
            Err(error) => tracing::warn!(%error, "could not draw the tray icon"),
        }
    });
}

/// Sets the rings as a colour image, or the template icon when there is none.
fn show(tray: &TrayIcon<Wry>, groups: &[Vec<Ring>]) -> tauri::Result<()> {
    match rings::render(groups) {
        Some(picture) => tray.set_icon_with_as_template(
            Some(Image::new_owned(
                picture.pixels,
                picture.width,
                picture.height,
            )),
            false,
        ),
        None => tray.set_icon_with_as_template(Some(Image::from_bytes(TRAY_ICON)?), true),
    }
}

/// Creates the tray icon with its menu.
///
/// # Errors
///
/// The icon or the menu cannot be built.
pub fn install(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(
                app,
                OPEN,
                format!("Open {DISPLAY_NAME}"),
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, SCAN, "Scan now", true, None::<&str>)?,
            &MenuItem::with_id(app, SETTINGS, "Settings", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                QUIT,
                format!("Quit {DISPLAY_NAME}"),
                true,
                None::<&str>,
            )?,
        ],
    )?;
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(TRAY_ICON)?)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .icon_as_template(true)
        .tooltip(DISPLAY_NAME)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                windows::toggle_recap(app, &app.state::<Shell>().recap_gate);
            }
        });
    builder.build(app)?;
    Ok(())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the signature `on_menu_event` requires"
)]
fn on_menu(app: &AppHandle<Wry>, event: MenuEvent) {
    match event.id().as_ref() {
        OPEN => open(app, Route::Overview),
        SETTINGS => open(app, Route::Settings),
        SCAN => {
            let shell = app.state::<Shell>().inner().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = actions::scan_now(&shell.backend).await {
                    tracing::warn!(%error, "scan from the tray menu failed");
                }
            });
        }
        QUIT => app.exit(0),
        _ => {}
    }
}

fn open(app: &AppHandle<Wry>, route: Route) {
    let target = DashboardTarget { route, agent: None };
    if let Err(error) = windows::show_main(app, &target) {
        tracing::warn!(%error, "could not open the main window");
    }
}

#[cfg(test)]
mod tests {
    use tokenmtr_core::{AgentId, ProfileId, QuotaEta};
    use tokenmtr_query::QuotaWindowView;

    use super::*;

    fn window(id: &str, used_percent: f64) -> QuotaWindowView {
        let window = WindowId::new(id).unwrap();
        QuotaWindowView {
            label: window.label(),
            window,
            used_percent,
            resets_at: None,
            resets_in_secs: None,
            eta: QuotaEta::NotEnoughData,
            spend: None,
        }
    }

    fn meter(profile: &str, status: QuotaStatus, windows: Vec<QuotaWindowView>) -> QuotaMeter {
        QuotaMeter {
            agent: AgentId::CLAUDE,
            profile: ProfileId::try_from(profile).unwrap(),
            fetched_at: Timestamp::UNIX_EPOCH,
            age_secs: 0,
            status,
            account: None,
            windows,
        }
    }

    fn menubar(rings: Vec<MenubarRing>, hidden: &[&str]) -> MenubarConfig {
        MenubarConfig {
            agents: vec![AgentId::CLAUDE],
            rings,
            hidden_profiles: hidden
                .iter()
                .map(|p| ProfileId::try_from(*p).unwrap())
                .collect(),
            shortcut: None,
            live_window: jiff::SignedDuration::from_mins(10),
        }
    }

    fn meters() -> Vec<QuotaMeter> {
        vec![
            meter(
                "default",
                QuotaStatus::Ok,
                vec![
                    window("five_hour", 12.0),
                    window("seven_day", 75.0),
                    window("seven_day_opus", 5.0),
                    window("weekly_scoped:fable", 95.0),
                    window("extra_usage", 50.0),
                ],
            ),
            meter("work", QuotaStatus::KeychainDenied, Vec::new()),
        ]
    }

    #[test]
    fn rings_follow_the_selection_per_profile() {
        let all = ring_groups(&meters(), &menubar(MenubarRing::ALL.to_vec(), &[]));
        assert_eq!(
            all,
            [
                vec![Ring::used(12.0), Ring::used(75.0), Ring::used(95.0)],
                vec![Ring::Unknown],
            ]
        );
        let per_model = ring_groups(&meters(), &menubar(vec![MenubarRing::PerModel], &["work"]));
        assert_eq!(per_model, [vec![Ring::used(95.0)]]);
    }

    const NONE: &[Vec<Ring>] = &[];

    #[test]
    fn no_selection_or_no_window_draws_nothing() {
        assert_eq!(ring_groups(&meters(), &menubar(Vec::new(), &[])), NONE);
        assert_eq!(
            ring_groups(&[], &menubar(MenubarRing::ALL.to_vec(), &[])),
            NONE
        );
        let only_session = vec![meter(
            "default",
            QuotaStatus::Ok,
            vec![window("seven_day", 1.0)],
        )];
        assert_eq!(
            ring_groups(&only_session, &menubar(vec![MenubarRing::Session], &[])),
            NONE
        );
    }

    #[test]
    fn rings_read_the_store() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = crate::backend::fixture::seeded(crate::backend::fixture::CONFIG)?;
        let app = fixture.backend.context()?;
        let ctx = app.query_context(fixture.now)?;
        let meters = tokenmtr_query::quota_latest(
            app.reader(),
            &ctx,
            &Filters::new(DateRange::day(ctx.today())?),
        )?;
        assert_eq!(ring_groups(&meters, &app.config().menubar), NONE);
        Ok(())
    }
}
