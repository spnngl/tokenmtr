//! The menu-bar item (D29): icon, title and native menu.
//!
//! The title text is computed by plain functions over [`TrayFigures`]; only [`install`] and
//! [`refresh`] touch Tauri.

use jiff::Timestamp;
use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager as _, Wry};
use tokenmtr_app::AppContext;
use tokenmtr_config::MenubarTitle;
use tokenmtr_core::Currency;
use tokenmtr_query::{Period, Selection};

use crate::backend::actions;
use crate::error::CommandError;
use crate::request::{DashboardTarget, Route};
use crate::state::Shell;
use crate::windows;

const TRAY_ID: &str = "tokenmtr";
/// Monochrome template image (44x44, black on transparent; `tray-icon.py`): macOS tints it.
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-icon@2x.png");
const OPEN: &str = "open";
const SCAN: &str = "scan";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

/// The numbers a tray title can show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrayFigures {
    /// Currency of the money figures.
    pub currency: Currency,
    /// What today costs in cash: pay-as-you-go plus prorated fees.
    pub paid_micros: i64,
    /// What today's usage is worth at API prices.
    pub api_value_micros: i64,
    /// The highest utilization among all quota windows.
    pub tightest_percent: Option<f64>,
}

/// Formats micro-units as `$12.34`, `€12.34`, or whole units from 100 up.
#[must_use]
pub fn format_money(micros: i64, currency: Currency) -> String {
    let symbol = match currency {
        Currency::Usd => '$',
        Currency::Eur => '€',
    };
    let cents = (micros.max(0).saturating_add(5_000)) / 10_000;
    if cents >= 10_000 {
        format!("{symbol}{}", (cents + 50) / 100)
    } else {
        format!("{symbol}{}.{:02}", cents / 100, cents % 100)
    }
}

/// The text next to the icon, `None` to show the icon alone.
#[must_use]
pub fn title_text(kind: MenubarTitle, figures: &TrayFigures) -> Option<String> {
    match kind {
        MenubarTitle::Hidden => None,
        MenubarTitle::PaidToday => Some(format_money(figures.paid_micros, figures.currency)),
        MenubarTitle::ApiToday => Some(format_money(figures.api_value_micros, figures.currency)),
        MenubarTitle::TightestQuota => figures
            .tightest_percent
            .map(|percent| format!("{}%", percent.round().max(0.0))),
    }
}

/// Reads today's figures across all agents.
///
/// # Errors
///
/// Store and query failures.
pub fn figures(app: &AppContext, now: Timestamp) -> Result<TrayFigures, CommandError> {
    let currency = app.config().display.currency;
    let ctx = tokenmtr_app::query_context(app.config(), app.reader(), now, Some(currency))?;
    let recap = tokenmtr_query::recap(
        app.reader(),
        &ctx,
        &app.recap_config(),
        Selection::All,
        Period::Today,
    )?
    .recap;
    let tightest_percent = recap
        .quota
        .iter()
        .flat_map(|meter| &meter.windows)
        .map(|window| window.used_percent)
        .reduce(f64::max);
    Ok(TrayFigures {
        currency: recap.currency,
        paid_micros: recap.totals.usage.paid_micros,
        api_value_micros: recap.totals.usage.api_value_micros,
        tightest_percent,
    })
}

/// The title for the current store and config.
///
/// # Errors
///
/// Config, store and query failures.
pub fn current_title(shell: &Shell, now: Timestamp) -> Result<Option<String>, CommandError> {
    let app = shell.backend.context()?;
    Ok(title_text(app.config().menubar.title, &figures(&app, now)?))
}

/// Recomputes the title off the main thread and applies it.
pub fn refresh(app: &AppHandle<Wry>) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let shell = handle.state::<Shell>().inner().clone();
        let title = crate::backend::blocking(move || current_title(&shell, Timestamp::now())).await;
        let Some(tray) = handle.tray_by_id(TRAY_ID) else {
            return;
        };
        let text = match title {
            Ok(text) => text,
            Err(error) => {
                tracing::debug!(%error, "tray title unavailable");
                return;
            }
        };
        if let Err(error) = tray.set_title(text) {
            tracing::warn!(%error, "could not set the tray title");
        }
    });
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
            &MenuItem::with_id(app, OPEN, "Open TokenMtr", true, None::<&str>)?,
            &MenuItem::with_id(app, SCAN, "Scan now", true, None::<&str>)?,
            &MenuItem::with_id(app, SETTINGS, "Settings", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, QUIT, "Quit TokenMtr", true, None::<&str>)?,
        ],
    )?;
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(TRAY_ICON)?)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .icon_as_template(true)
        .tooltip("TokenMtr")
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
    use super::*;

    const FIGURES: TrayFigures = TrayFigures {
        currency: Currency::Usd,
        paid_micros: 12_345_678,
        api_value_micros: 187_000_000,
        tightest_percent: Some(81.6),
    };

    #[test]
    fn money_has_cents_below_a_hundred_and_whole_units_above() {
        assert_eq!(format_money(0, Currency::Usd), "$0.00");
        assert_eq!(format_money(12_345_678, Currency::Usd), "$12.35");
        assert_eq!(format_money(99_994_999, Currency::Eur), "€99.99");
        assert_eq!(format_money(187_000_000, Currency::Usd), "$187");
        assert_eq!(format_money(-5, Currency::Usd), "$0.00");
    }

    #[test]
    fn title_follows_the_configured_kind() {
        assert_eq!(
            title_text(MenubarTitle::PaidToday, &FIGURES).as_deref(),
            Some("$12.35")
        );
        assert_eq!(
            title_text(MenubarTitle::ApiToday, &FIGURES).as_deref(),
            Some("$187")
        );
        assert_eq!(
            title_text(MenubarTitle::TightestQuota, &FIGURES).as_deref(),
            Some("82%")
        );
        assert_eq!(title_text(MenubarTitle::Hidden, &FIGURES), None);
        let no_quota = TrayFigures {
            tightest_percent: None,
            ..FIGURES
        };
        assert_eq!(title_text(MenubarTitle::TightestQuota, &no_quota), None);
    }

    #[test]
    fn figures_read_today_from_the_store() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = crate::backend::fixture::seeded(crate::backend::fixture::CONFIG)?;
        let app = fixture.backend.context()?;
        let figures = figures(&app, fixture.now)?;
        assert_eq!(figures.api_value_micros, 5_000_000);
        assert_eq!(figures.tightest_percent, None);
        Ok(())
    }
}
