//! The Tauri commands: one per `DataSource` method of the UI (`shared/data-source.ts`).
//!
//! Each command is a thin wrapper: parse the arguments, run a plain function of
//! [`crate::backend`] on a worker thread, return JSON the UI's Zod schemas parse. The command
//! names are the method names in `snake_case`; `command_names.in` lists them once for the
//! permission manifest (`build.rs`), the invoke handler and the generated `bindings.ts`.

#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri deserialises command arguments by value and `State` is owned"
)]

use camino::Utf8PathBuf;
use jiff::Timestamp;
use serde_json::Value;
use tauri::{AppHandle, State, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tokenmtr_app::AppContext;
use tokenmtr_config::RawProject;
use tokenmtr_core::{AgentId, Currency, ProfileId, SessionId, SubscriptionSpec};
use tokenmtr_platform::service::is_transient;
use tokenmtr_platform::{
    LaunchdService, ServiceError, ServiceManager as _, ServiceStatus, SystemRunner,
};
use tokenmtr_query::{Granularity, GroupBy, Period};

use crate::backend::shell::{
    self, AppState, FilterOptions, Permission, SessionOrigin, Settings, StateInputs,
};
use crate::backend::{actions, blocking, queries};
use crate::daemon::{Link, Sidecar, probe};
use crate::error::CommandError;
use crate::request::{QueryRequest, ReportWire, RevealTarget, Route};
use crate::state::Shell;
use crate::windows;

type Reply<T> = Result<T, CommandError>;

/// Runs `work` against a fresh context on a worker thread.
async fn read<T, F>(shell: &Shell, work: F) -> Reply<T>
where
    T: Send + 'static,
    F: FnOnce(&AppContext, Timestamp) -> Reply<T> + Send + 'static,
{
    let backend = shell.backend.clone();
    blocking(move || work(&backend.context()?, Timestamp::now())).await
}

fn permission(app: &AppHandle<Wry>) -> Permission {
    use tauri_plugin_notification::PermissionState;
    match app.notification().permission_state() {
        Ok(PermissionState::Granted) => Permission::Granted,
        Ok(PermissionState::Denied) => Permission::Denied,
        Ok(_) | Err(_) => Permission::Undetermined,
    }
}

// --- shell shapes -------------------------------------------------------------------------

/// Where the app is in its life.
#[tauri::command]
pub async fn app_state(app: AppHandle<Wry>, shell: State<'_, Shell>) -> Reply<AppState> {
    let link = probe(shell.backend.transport()).await;
    let onboarding_complete = shell.store.onboarding_complete();
    let notifications = permission(&app);
    let backend = shell.backend.clone();
    blocking(move || {
        let daemon_installed = !onboarding_complete
            && !matches!(
                LaunchdService::new(backend.paths(), SystemRunner).status(),
                Ok(ServiceStatus::NotInstalled) | Err(_)
            );
        shell::app_state(
            || backend.context(),
            StateInputs {
                onboarding_complete,
                link,
                daemon_installed,
                notifications,
            },
        )
    })
    .await
}

/// Values the filter bar offers.
#[tauri::command]
pub async fn filter_options(shell: State<'_, Shell>) -> Reply<FilterOptions> {
    read(&shell, |app, _| shell::filter_options(app)).await
}

/// The Settings page.
#[tauri::command]
pub async fn settings(app: AppHandle<Wry>, shell: State<'_, Shell>) -> Reply<Settings> {
    let daemon_version = match probe(shell.backend.transport()).await {
        Link::Connected { daemon_version } | Link::Incompatible { daemon_version, .. } => {
            daemon_version
        }
        Link::Offline => String::new(),
    };
    let launch_at_login = app.autolaunch().is_enabled().unwrap_or(false);
    read(&shell, move |ctx, now| {
        shell::settings(ctx, now, daemon_version, launch_at_login)
    })
    .await
}

/// Where a session lives on disk.
#[tauri::command]
pub async fn session_origin(
    shell: State<'_, Shell>,
    agent: AgentId,
    session: SessionId,
) -> Reply<SessionOrigin> {
    read(&shell, move |ctx, _| {
        shell::session_origin(ctx, agent, &session)
    })
    .await
}

// --- queries ------------------------------------------------------------------------------

macro_rules! filtered_query {
    ($($name:ident),* $(,)?) => {$(
        #[doc = concat!("`", stringify!($name), "` over the filtered range.")]
        #[tauri::command]
        pub async fn $name(shell: State<'_, Shell>, request: QueryRequest) -> Reply<Value> {
            read(&shell, move |app, now| queries::$name(app, now, &request)).await
        }
    )*};
}

filtered_query!(
    overview,
    models,
    projects,
    branches,
    unresolved_cwds,
    heatmap,
    tools,
    signals,
    billing,
    quota_latest,
    advisor,
    session_analytics,
);

/// Spend over time in hourly, daily or monthly buckets, optionally per agent.
#[tauri::command]
pub async fn billing_series(
    shell: State<'_, Shell>,
    request: QueryRequest,
    granularity: Granularity,
    split_by_agent: bool,
) -> Reply<Value> {
    read(&shell, move |app, now| {
        queries::billing_series(app, now, &request, granularity, split_by_agent)
    })
    .await
}

/// Usage over time, optionally grouped.
#[tauri::command]
pub async fn series(
    shell: State<'_, Shell>,
    request: QueryRequest,
    group_by: GroupBy,
) -> Reply<Value> {
    read(&shell, move |app, now| {
        queries::series(app, now, &request, group_by)
    })
    .await
}

/// One page of sessions.
#[tauri::command]
pub async fn sessions(
    shell: State<'_, Shell>,
    request: QueryRequest,
    page: u32,
    page_size: u32,
) -> Reply<Value> {
    read(&shell, move |app, now| {
        queries::sessions(app, now, &request, page, page_size)
    })
    .await
}

/// Quota polls of one agent over the range.
#[tauri::command]
pub async fn quota_history(
    shell: State<'_, Shell>,
    request: QueryRequest,
    agent: AgentId,
) -> Reply<Value> {
    read(&shell, move |app, now| {
        queries::quota_history(app, now, &request, agent)
    })
    .await
}

/// Events, tool calls and signals of one session.
#[tauri::command]
pub async fn session_detail(
    shell: State<'_, Shell>,
    agent: AgentId,
    session: SessionId,
    currency: Currency,
) -> Reply<Value> {
    read(&shell, move |app, now| {
        queries::session_detail(app, now, agent, &session, currency)
    })
    .await
}

/// Cumulative spend, signals and tool counts of one session.
#[tauri::command]
pub async fn session_timeline(
    shell: State<'_, Shell>,
    agent: AgentId,
    session: SessionId,
    currency: Currency,
) -> Reply<Value> {
    read(&shell, move |app, now| {
        queries::session_timeline(app, now, agent, &session, currency)
    })
    .await
}

/// Budgets and how much of each is used.
#[tauri::command]
pub async fn budgets(shell: State<'_, Shell>, currency: Currency) -> Reply<Value> {
    read(&shell, move |app, now| queries::budgets(app, now, currency)).await
}

/// The tray window's tabs.
#[tauri::command]
pub async fn recap(shell: State<'_, Shell>, period: Period, currency: Currency) -> Reply<Value> {
    read(&shell, move |app, now| {
        queries::recap(app, now, currency, period)
    })
    .await
}

/// Daemon and sinks health.
#[tauri::command]
pub async fn system_health(shell: State<'_, Shell>) -> Reply<Value> {
    read(&shell, queries::system_health).await
}

/// Per-agent ingest runs.
#[tauri::command]
pub async fn ingest_status(shell: State<'_, Shell>) -> Reply<Value> {
    read(&shell, |app, _| queries::ingest_status(app)).await
}

/// Open parse issues.
#[tauri::command]
pub async fn parse_issues(shell: State<'_, Shell>) -> Reply<Value> {
    read(&shell, |app, _| queries::parse_issues(app)).await
}

/// Recent alerts.
#[tauri::command]
pub async fn alerts(shell: State<'_, Shell>) -> Reply<Value> {
    read(&shell, queries::alerts).await
}

/// Recent backup attempts.
#[tauri::command]
pub async fn backups(shell: State<'_, Shell>) -> Reply<Value> {
    read(&shell, |app, _| queries::backups(app)).await
}

/// Stored FX rates.
#[tauri::command]
pub async fn fx_rates(shell: State<'_, Shell>) -> Reply<Value> {
    read(&shell, queries::fx_rates).await
}

// --- actions ------------------------------------------------------------------------------

/// Scans every agent now.
#[tauri::command]
pub async fn scan_now(shell: State<'_, Shell>) -> Reply<()> {
    actions::scan_now(&shell.backend).await
}

/// Runs the export sinks now.
#[tauri::command]
pub async fn export_now(shell: State<'_, Shell>) -> Reply<()> {
    actions::export_now(&shell.backend).await
}

/// Writes a backup now.
#[tauri::command]
pub async fn backup_now(shell: State<'_, Shell>) -> Reply<()> {
    actions::backup_now(&shell.backend).await
}

/// Opens the main window at a page, optionally filtered to an agent.
#[tauri::command]
pub fn open_dashboard(app: AppHandle<Wry>, route: Route, agent: Option<AgentId>) -> Reply<()> {
    windows::show_main(&app, &crate::request::DashboardTarget { route, agent })
        .map_err(CommandError::internal)
}

/// Installs the background service.
#[tauri::command]
pub async fn install_daemon(shell: State<'_, Shell>) -> Reply<()> {
    let backend = shell.backend.clone();
    blocking(move || actions::install_daemon(&backend)).await
}

/// Points the service at the bundled binary and restarts it.
#[tauri::command]
pub async fn restart_daemon(shell: State<'_, Shell>) -> Reply<()> {
    install_daemon(shell).await
}

/// Shows one of the places the app knows in Finder.
#[tauri::command]
pub async fn reveal_path(shell: State<'_, Shell>, target: RevealTarget) -> Reply<()> {
    let backend = shell.backend.clone();
    let path = blocking(move || actions::reveal_path(&backend, &target)).await?;
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(CommandError::internal)
}

/// Labels the sessions started in a directory with a project.
#[tauri::command]
pub async fn add_project_rule(
    app: AppHandle<Wry>,
    shell: State<'_, Shell>,
    cwd: String,
    name: String,
) -> Reply<()> {
    let backend = shell.backend.clone();
    blocking({
        let backend = backend.clone();
        move || actions::add_project_rule(&backend, &cwd, &name)
    })
    .await?;
    actions::reload_daemon(&backend).await;
    crate::shortcut::sync(&app);
    Ok(())
}

/// Records a subscription.
#[tauri::command]
pub async fn add_subscription(
    app: AppHandle<Wry>,
    shell: State<'_, Shell>,
    spec: SubscriptionSpec,
) -> Reply<()> {
    let backend = shell.backend.clone();
    blocking({
        let backend = backend.clone();
        move || actions::add_subscription(&backend, &spec)
    })
    .await?;
    actions::reload_daemon(&backend).await;
    crate::shortcut::sync(&app);
    Ok(())
}

/// The project rule the app suggests for a directory.
#[tauri::command]
pub async fn project_rule_for(shell: State<'_, Shell>, cwd: String) -> Reply<RawProject> {
    let backend = shell.backend.clone();
    blocking(move || actions::project_rule_for(&backend, &cwd, Timestamp::now())).await
}

/// Lets the daemon read the Claude credentials in the Keychain: macOS asks the user once.
#[tauri::command]
pub async fn connect_keychain(shell: State<'_, Shell>) -> Reply<()> {
    actions::connect_keychain(&shell.backend).await
}

/// Raises a Claude profile's transcript retention to ten years.
#[tauri::command]
pub async fn keep_ten_years(
    shell: State<'_, Shell>,
    agent: AgentId,
    profile: ProfileId,
) -> Reply<()> {
    let backend = shell.backend.clone();
    blocking(move || actions::keep_ten_years(&backend, agent, &profile)).await
}

/// Starts (or stops starting) the app at login. Refused while the app runs from a disk image or
/// a translocated copy: the login item would point at a path that disappears.
#[tauri::command]
pub fn set_launch_at_login(app: AppHandle<Wry>, enabled: bool) -> Reply<()> {
    let autostart = app.autolaunch();
    let result = if enabled {
        let sidecar = Sidecar::locate()?;
        if is_transient(sidecar.path()) {
            return Err(ServiceError::TransientBinary(sidecar.path().to_owned()).into());
        }
        autostart.enable()
    } else {
        autostart.disable()
    };
    result.map_err(CommandError::internal)
}

/// Deletes the store, backups, logs and the background service; onboarding starts over.
#[tauri::command]
pub async fn erase_all_data(shell: State<'_, Shell>) -> Reply<()> {
    actions::erase_all_data(&shell.backend).await?;
    shell.store.set_onboarding_complete(false)
}

/// Marks onboarding as done.
#[tauri::command]
pub fn complete_onboarding(shell: State<'_, Shell>) -> Reply<()> {
    shell.store.set_onboarding_complete(true)
}

/// Asks for notification permission. macOS prompts on the first notification, and the plugin
/// cannot query it before, so this reports what the plugin reports.
#[tauri::command]
pub fn request_notifications(app: AppHandle<Wry>) -> Reply<Permission> {
    use tauri_plugin_notification::PermissionState;
    match app
        .notification()
        .request_permission()
        .map_err(CommandError::internal)?
    {
        PermissionState::Granted => Ok(Permission::Granted),
        PermissionState::Denied => Ok(Permission::Denied),
        _ => Ok(Permission::Undetermined),
    }
}

// --- reports ------------------------------------------------------------------------------

/// Builds a client report.
#[tauri::command]
pub async fn generate_report(
    shell: State<'_, Shell>,
    request: ReportWire,
) -> Reply<actions::GeneratedReport> {
    let backend = shell.backend.clone();
    blocking(move || actions::generate_report(&backend, Timestamp::now(), &request.into())).await
}

/// Builds a client report and saves it where the user chooses; `None` when they cancel.
#[tauri::command]
pub async fn save_report(
    app: AppHandle<Wry>,
    shell: State<'_, Shell>,
    request: ReportWire,
) -> Reply<Option<String>> {
    let backend = shell.backend.clone();
    blocking(move || {
        let report = actions::generate_report(&backend, Timestamp::now(), &request.into())?;
        let Some(file) = app
            .dialog()
            .file()
            .set_file_name(&report.filename)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(CommandError::internal)?;
        let path = Utf8PathBuf::from_path_buf(path)
            .map_err(|path| CommandError::invalid(format!("{} is not UTF-8", path.display())))?;
        // Atomic: a failed write never leaves half a report over an existing file.
        tokenmtr_platform::fs::replace_atomically(&path, report.content.as_bytes())
            .map_err(|error| CommandError::internal(format!("cannot write {path}: {error}")))?;
        Ok(Some(path.into_string()))
    })
    .await
}

/// Every command name: what `build.rs` declares to Tauri's permission system and what
/// `bindings.ts` lists. A test checks it against the invoke handler's table.
pub const NAMES: &[&str] = include!("command_names.in");

/// Declares the invoke handler.
macro_rules! command_table {
    ($($name:ident),* $(,)?) => {
        /// The names the invoke handler routes, in its own order.
        #[cfg(test)]
        const ROUTED: &[&str] = &[$(stringify!($name)),*];

        /// The invoke handler routing every command.
        pub fn handler() -> impl Fn(tauri::ipc::Invoke<Wry>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$($name),*]
        }
    };
}

command_table!(
    app_state,
    filter_options,
    settings,
    session_origin,
    overview,
    series,
    models,
    projects,
    branches,
    unresolved_cwds,
    heatmap,
    tools,
    signals,
    billing,
    billing_series,
    quota_latest,
    quota_history,
    advisor,
    sessions,
    session_detail,
    session_analytics,
    session_timeline,
    budgets,
    recap,
    system_health,
    ingest_status,
    parse_issues,
    alerts,
    backups,
    fx_rates,
    scan_now,
    export_now,
    backup_now,
    open_dashboard,
    install_daemon,
    restart_daemon,
    reveal_path,
    add_project_rule,
    add_subscription,
    project_rule_for,
    connect_keychain,
    keep_ten_years,
    set_launch_at_login,
    erase_all_data,
    complete_onboarding,
    request_notifications,
    generate_report,
    save_report,
);

#[cfg(test)]
mod tests {
    use super::{NAMES, ROUTED};

    #[test]
    fn declared_names_are_the_routed_commands() {
        assert_eq!(NAMES, ROUTED);
    }

    #[test]
    fn names_are_unique() {
        let mut sorted = NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), NAMES.len());
    }
}
