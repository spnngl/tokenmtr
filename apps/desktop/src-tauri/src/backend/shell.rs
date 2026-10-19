//! The shapes only the shell composes: app state, settings, filter options, session origin.
//!
//! They follow `apps/desktop/src/shared/schemas/app.ts` field for field.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tokenmtr_agents::ingest::DoctorReport;
use tokenmtr_app::{AppContext, RETENTION_DAYS};
use tokenmtr_config::{Config, MenubarTitle, ProfileLocation};
use tokenmtr_core::{AgentId, SessionId};
use tokenmtr_query::{BackupView, DaemonHealth, FxRateView, QuotaMeter};
use tokenmtr_store::StoreRead as _;

use crate::daemon::Link;
use crate::error::CommandError;

/// What an agent can report; fixed per agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// Quota windows can be polled.
    pub quota: bool,
    /// The agent reports its own cost.
    pub reported_cost: bool,
    /// Usage can be covered by a subscription.
    pub subscription: bool,
}

/// The capabilities of `agent`.
#[must_use]
pub fn capabilities(agent: AgentId) -> Capabilities {
    if agent == AgentId::CLAUDE {
        Capabilities {
            quota: true,
            reported_cost: false,
            subscription: true,
        }
    } else {
        Capabilities {
            quota: false,
            reported_cost: agent == AgentId::PI,
            subscription: true,
        }
    }
}

fn label(agent: AgentId) -> String {
    if agent == AgentId::CLAUDE {
        "Claude Code".to_owned()
    } else {
        agent.as_str().to_owned()
    }
}

/// One profile as found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileInfo {
    /// Profile id.
    pub profile: String,
    /// Its first root (or config directory).
    pub root: String,
    /// The account the latest quota poll belongs to.
    pub account: Option<String>,
    /// Days after which the agent deletes transcripts.
    pub retention_days: Option<u64>,
    /// Retention already raised to ten years.
    pub keeps_ten_years: bool,
}

/// One agent with its profiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentInfo {
    /// Agent id.
    pub agent: String,
    /// Display name.
    pub label: String,
    /// Has a recap tab (`menubar.agents`).
    pub followed: bool,
    /// What it can report.
    pub capabilities: Capabilities,
    /// Its profiles.
    pub profiles: Vec<ProfileInfo>,
}

/// Where the app is in its life.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppState {
    /// Daemon connected: the screens show data.
    Ready {
        /// End of the newest ingest run.
        last_scan_at: Option<Timestamp>,
        /// Safety-net sweep interval.
        scan_interval_secs: u64,
        /// Agents whose collector has open issues.
        failed_collectors: u64,
    },
    /// First run.
    Onboarding {
        /// Agents found.
        agents: Vec<AgentInfo>,
        /// A launchd plist exists.
        daemon_installed: bool,
        /// Notification permission.
        notifications: Permission,
    },
    /// Nobody answers on the control socket.
    DaemonOffline {},
    /// App and daemon disagree on protocol or schema.
    Incompatible {
        /// This app's version.
        app_version: String,
        /// The daemon's.
        daemon_version: String,
    },
}

/// Notification permission as the UI knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Allowed.
    Granted,
    /// Refused.
    Denied,
    /// Not asked yet.
    Undetermined,
}

/// What the shell knows from outside the store when it composes the state.
#[derive(Debug, Clone)]
pub struct StateInputs {
    /// The user finished onboarding.
    pub onboarding_complete: bool,
    /// The latest handshake.
    pub link: Link,
    /// A plist is installed.
    pub daemon_installed: bool,
    /// Notification permission.
    pub notifications: Permission,
}

/// The state of the app: onboarding first, then the daemon link, then ready.
///
/// # Errors
///
/// Config or store failures while the state needs them (onboarding, ready).
pub fn app_state(
    app: impl FnOnce() -> Result<AppContext, CommandError>,
    inputs: StateInputs,
) -> Result<AppState, CommandError> {
    if !inputs.onboarding_complete {
        let app = app()?;
        let doctor = app.ingest_report().ok();
        return Ok(AppState::Onboarding {
            agents: agent_infos(&app, doctor.as_ref(), &[]),
            daemon_installed: inputs.daemon_installed,
            notifications: inputs.notifications,
        });
    }
    match inputs.link {
        Link::Incompatible { daemon_version } => Ok(AppState::Incompatible {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            daemon_version,
        }),
        Link::Offline => Ok(AppState::DaemonOffline {}),
        Link::Connected { .. } => {
            let app = app()?;
            let runs = tokenmtr_query::ingest_status(app.reader())?;
            Ok(AppState::Ready {
                last_scan_at: runs.iter().map(|run| run.last_run_at).max(),
                scan_interval_secs: u64::try_from(app.config().daemon.scan_interval.as_secs())
                    .unwrap_or(0),
                failed_collectors: runs.iter().filter(|run| run.open_issues > 0).count() as u64,
            })
        }
    }
}

fn first_root(
    doctor: Option<&DoctorReport>,
    agent: AgentId,
    profile: &tokenmtr_core::ProfileId,
    location: &ProfileLocation,
) -> String {
    let found = doctor
        .and_then(|report| report.agents.iter().find(|a| a.agent == agent))
        .and_then(|report| report.profiles.iter().find(|p| p.profile == *profile))
        .and_then(|report| report.roots.first())
        .map(|root| root.path.to_string());
    found.unwrap_or_else(|| match location {
        ProfileLocation::Roots(roots) => roots.first().map(ToString::to_string).unwrap_or_default(),
        ProfileLocation::ConfigDir(dir) => dir.to_string(),
        ProfileLocation::Default => String::new(),
    })
}

/// The enabled agents with their profiles, retention and accounts.
#[must_use]
pub fn agent_infos(
    app: &AppContext,
    doctor: Option<&DoctorReport>,
    meters: &[QuotaMeter],
) -> Vec<AgentInfo> {
    let config = app.config();
    config
        .agents
        .iter()
        .filter(|(_, agent)| agent.enabled)
        .map(|(id, agent)| AgentInfo {
            agent: id.as_str().to_owned(),
            label: label(*id),
            followed: config.menubar.agents.contains(id),
            capabilities: capabilities(*id),
            profiles: agent
                .profiles
                .iter()
                .map(|profile| {
                    let retention = doctor
                        .and_then(|report| report.agents.iter().find(|a| a.agent == *id))
                        .and_then(|report| report.profiles.iter().find(|p| p.profile == profile.id))
                        .and_then(|report| report.retention);
                    ProfileInfo {
                        profile: profile.id.as_str().to_owned(),
                        root: first_root(doctor, *id, &profile.id, &profile.location),
                        account: meters
                            .iter()
                            .find(|m| m.agent == *id && m.profile == profile.id)
                            .and_then(|m| m.account.clone()),
                        retention_days: retention.map(|r| r.cleanup_period_days),
                        keeps_ten_years: retention.is_some_and(|r| {
                            r.configured && r.cleanup_period_days >= RETENTION_DAYS
                        }),
                    }
                })
                .collect(),
        })
        .collect()
}

/// The daemon block of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonBlock {
    /// Liveness from the heartbeat.
    #[serde(flatten)]
    pub health: DaemonHealth,
    /// Version the handshake reported; empty when no daemon answers.
    pub version: String,
}

/// Display preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DisplayBlock {
    /// Currency shown by default.
    pub currency: tokenmtr_core::Currency,
    /// IANA zone that buckets days.
    pub time_zone: String,
}

/// FX state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FxBlock {
    /// Override rate, EUR per USD.
    pub fixed_eur_per_usd: Option<f64>,
    /// Newest stored rate.
    pub latest: Option<FxRateView>,
    /// No rate available at all, so EUR amounts fall back to USD.
    pub offline: bool,
}

/// Pricing snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PricingBlock {
    /// Day the snapshot was fetched (`embedded` for the bundled one).
    pub snapshot_date: String,
    /// Priced models in it.
    pub models: usize,
}

/// Tray preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrayBlock {
    /// `paid_today`, `api_today`, `tightest_quota` or `none`.
    pub title: String,
    /// Global shortcut; empty when disabled.
    pub shortcut: String,
}

/// Alert rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlertsBlock {
    /// Quota thresholds, percent.
    pub quota_thresholds: Vec<u8>,
    /// Budget thresholds, percent.
    pub budget_thresholds: Vec<u8>,
    /// Announce quota resets.
    pub quota_reset: bool,
}

/// An agent folder macOS guards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtectedFolder {
    /// Agent that reads it.
    pub agent: String,
    /// The folder.
    pub path: String,
    /// The app could read it.
    pub accessible: bool,
}

/// The Settings page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    /// Config file.
    pub config_path: String,
    /// Default exports directory.
    pub exports_dir: String,
    /// Machine label.
    pub machine: String,
    /// Agents.
    pub agents: Vec<AgentInfo>,
    /// Daemon.
    pub daemon: DaemonBlock,
    /// Display.
    pub display: DisplayBlock,
    /// FX.
    pub fx: FxBlock,
    /// Pricing.
    pub pricing: PricingBlock,
    /// Tray.
    pub tray: TrayBlock,
    /// Alert rules.
    pub alerts: AlertsBlock,
    /// The app starts at login.
    pub launch_at_login: bool,
    /// Protected agent folders.
    pub protected_folders: Vec<ProtectedFolder>,
    /// Recent backups.
    pub backups: Vec<BackupView>,
}

/// The text of a tray title kind, as the config spells it.
#[must_use]
pub const fn title_name(title: MenubarTitle) -> &'static str {
    match title {
        MenubarTitle::PaidToday => "paid_today",
        MenubarTitle::ApiToday => "api_today",
        MenubarTitle::TightestQuota => "tightest_quota",
        MenubarTitle::Hidden => "none",
    }
}

fn machine_name(config: &Config) -> String {
    config
        .machine
        .name
        .clone()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "this Mac".to_owned())
}

/// The settings, composed from the config, the store and the doctor.
///
/// # Errors
///
/// Store and query failures.
pub fn settings(
    app: &AppContext,
    now: Timestamp,
    daemon_version: String,
    launch_at_login: bool,
) -> Result<Settings, CommandError> {
    let config = app.config();
    let ctx = app.query_context(now)?;
    let quota = app.quota(now)?;
    let doctor = app.ingest_report().ok();
    let fx = app.fx(now)?;
    let pricing = app.pricing(now)?;
    let snapshot_date = pricing.snapshot.as_ref().map_or_else(
        || "embedded".to_owned(),
        |snapshot| {
            snapshot
                .fetched_at
                .to_zoned(jiff::tz::TimeZone::UTC)
                .date()
                .to_string()
        },
    );
    let models = app
        .reader()
        .active_pricing_snapshot()?
        .and_then(|snapshot| tokenmtr_core::PricingCatalog::parse(&snapshot.body).ok())
        .or_else(|| tokenmtr_core::PricingCatalog::embedded().ok())
        .map_or(0, |catalog| catalog.len());
    let protected_folders = doctor
        .iter()
        .flat_map(|report| &report.agents)
        .flat_map(|agent| {
            agent.profiles.iter().flat_map(move |profile| {
                profile
                    .roots
                    .iter()
                    .filter(|root| app.paths().is_protected(&root.path))
                    .map(move |root| ProtectedFolder {
                        agent: agent.agent.as_str().to_owned(),
                        path: root.path.to_string(),
                        accessible: root.exists,
                    })
            })
        })
        .collect();
    let zone = tokenmtr_app::display_zone(config);
    Ok(Settings {
        config_path: app.config_path().to_string(),
        exports_dir: app.paths().exports_dir().to_string(),
        machine: machine_name(config),
        agents: agent_infos(app, doctor.as_ref(), &quota.meters),
        daemon: DaemonBlock {
            health: tokenmtr_query::daemon_health(
                app.reader(),
                &ctx,
                config.alerts.daemon_stale_after,
            )?,
            version: daemon_version,
        },
        display: DisplayBlock {
            currency: config.display.currency,
            time_zone: zone.iana_name().unwrap_or("UTC").to_owned(),
        },
        fx: FxBlock {
            fixed_eur_per_usd: fx.fixed_eur_per_usd_scaled.map(unscale),
            offline: fx.fixed_eur_per_usd_scaled.is_none() && fx.latest.is_none(),
            latest: fx.latest,
        },
        pricing: PricingBlock {
            snapshot_date,
            models,
        },
        tray: TrayBlock {
            title: title_name(config.menubar.title).to_owned(),
            shortcut: config.menubar.shortcut.clone().unwrap_or_default(),
        },
        alerts: AlertsBlock {
            quota_thresholds: config.alerts.rules.quota_thresholds.clone(),
            budget_thresholds: config.alerts.rules.budget_thresholds.clone(),
            quota_reset: config.alerts.rules.quota_reset,
        },
        launch_at_login,
        protected_folders,
        backups: app.backup_history(20)?,
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "EUR per USD scaled by 1e9 is far below 2^52"
)]
fn unscale(scaled: i64) -> f64 {
    scaled as f64 / 1e9
}

/// Values the filter bar offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FilterOptions {
    /// Agents.
    pub agents: Vec<String>,
    /// Profiles as `agent/profile`: profile names are unique only within an agent.
    pub profiles: Vec<String>,
    /// Models.
    pub models: Vec<String>,
    /// Projects.
    pub projects: Vec<String>,
    /// Clients.
    pub clients: Vec<String>,
}

/// The distinct non-null values of `expression` (a column or an SQL expression over columns).
/// Extra-usage rows carry a placeholder model and session, never a filter value.
fn distinct(app: &AppContext, expression: &'static str) -> Result<Vec<String>, CommandError> {
    let sql = format!(
        "SELECT DISTINCT {expression} AS value FROM usage_event \
         WHERE value IS NOT NULL AND kind <> 'extra_usage' ORDER BY value"
    );
    let mut statement = app
        .reader()
        .connection()
        .prepare(&sql)
        .map_err(store_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(store_error)?;
    rows.collect::<Result<_, _>>().map_err(store_error)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "passed to `map_err`, which hands the error over by value"
)]
fn store_error(error: rusqlite::Error) -> CommandError {
    CommandError::new(crate::error::ErrorKind::Store, error.to_string())
}

/// The distinct values of every filter dimension in the store.
///
/// # Errors
///
/// SQL failures.
pub fn filter_options(app: &AppContext) -> Result<FilterOptions, CommandError> {
    Ok(FilterOptions {
        agents: distinct(app, "agent")?,
        profiles: distinct(app, "agent || '/' || profile")?,
        models: distinct(app, "model")?,
        projects: distinct(app, "project")?,
        clients: distinct(app, "client")?,
    })
}

/// Where a session lives on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionOrigin {
    /// Working directory of the session's latest event.
    pub cwd: Option<String>,
    /// The agent session root holding it when that is not the agent's default one.
    pub session_dir: Option<String>,
    /// The transcript file.
    pub transcript_path: Option<String>,
    /// The transcript still exists.
    pub transcript_exists: bool,
}

/// The origin of one session.
///
/// # Errors
///
/// SQL failures.
pub fn session_origin(
    app: &AppContext,
    agent_id: AgentId,
    session: &SessionId,
) -> Result<SessionOrigin, CommandError> {
    let row: Option<(Option<String>, Option<String>)> = app
        .reader()
        .connection()
        .query_row(
            "SELECT e.cwd, f.path FROM usage_event e \
             LEFT JOIN source_file f ON f.id = e.source_file_id \
             WHERE e.agent = ?1 AND e.session = ?2 ORDER BY e.at_us DESC LIMIT 1",
            rusqlite::params![agent_id.as_str(), session.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(store_error)?;
    let (cwd, transcript) = row.unwrap_or((None, None));
    let transcript_exists = transcript
        .as_deref()
        .is_some_and(|path| Utf8Path::new(path).is_file());
    let session_dir = transcript
        .as_deref()
        .and_then(|path| explicit_root(app.config(), agent_id, Utf8Path::new(path)))
        .map(|root| root.to_string());
    Ok(SessionOrigin {
        cwd,
        session_dir,
        transcript_path: transcript,
        transcript_exists,
    })
}

/// The explicitly configured session root of `agent` that holds `transcript`.
fn explicit_root(config: &Config, agent: AgentId, transcript: &Utf8Path) -> Option<Utf8PathBuf> {
    config
        .agents
        .get(&agent)?
        .profiles
        .iter()
        .filter_map(|profile| match &profile.location {
            ProfileLocation::Roots(roots) => Some(roots),
            ProfileLocation::Default | ProfileLocation::ConfigDir(_) => None,
        })
        .flatten()
        .find(|root| transcript.starts_with(root))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fixture::{CONFIG, seeded};

    fn inputs(complete: bool, link: Link) -> StateInputs {
        StateInputs {
            onboarding_complete: complete,
            link,
            daemon_installed: false,
            notifications: Permission::Undetermined,
        }
    }

    #[test]
    fn state_machine_orders_onboarding_then_link() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let backend = fixture.backend.clone();
        let load = || backend.context();
        let connected = Link::Connected {
            daemon_version: "1.0.0".to_owned(),
        };
        let state = app_state(load, inputs(false, Link::Offline))?;
        assert!(matches!(state, AppState::Onboarding { .. }), "{state:?}");
        let state = app_state(load, inputs(true, Link::Offline))?;
        assert_eq!(state, AppState::DaemonOffline {});
        let state = app_state(
            load,
            inputs(
                true,
                Link::Incompatible {
                    daemon_version: "9.0.0".to_owned(),
                },
            ),
        )?;
        assert!(
            matches!(&state, AppState::Incompatible { daemon_version, .. } if daemon_version == "9.0.0"),
            "{state:?}"
        );
        let state = app_state(load, inputs(true, connected))?;
        assert!(matches!(state, AppState::Ready { .. }), "{state:?}");
        Ok(())
    }

    #[test]
    fn offline_state_needs_no_store() -> Result<(), CommandError> {
        let state = app_state(
            || Err(CommandError::internal("must not open the store")),
            inputs(true, Link::Offline),
        )?;
        assert_eq!(state, AppState::DaemonOffline {});
        Ok(())
    }

    #[test]
    fn app_state_serialises_with_a_kind() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::to_value(AppState::DaemonOffline {})?,
            serde_json::json!({ "kind": "daemon_offline" })
        );
        Ok(())
    }

    #[test]
    fn filter_options_list_the_stored_values() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let options = filter_options(&app)?;
        assert_eq!(options.agents, ["claude"]);
        assert_eq!(options.profiles, ["claude/default"]);
        assert_eq!(options.models, ["claude-sonnet-4"]);
        assert_eq!(options.projects, ["acme"]);
        assert_eq!(options.clients.len(), 0);
        Ok(())
    }

    #[test]
    fn settings_compose_config_and_store() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let settings = settings(&app, fixture.now, "1.2.3".to_owned(), true)?;
        assert_eq!(settings.display.time_zone, "UTC");
        assert_eq!(settings.daemon.version, "1.2.3");
        assert!(settings.launch_at_login);
        assert!(settings.fx.offline);
        assert!(settings.agents.iter().any(|a| a.agent == "claude"));
        let json = serde_json::to_value(&settings)?;
        assert_eq!(json["daemon"]["state"], "never_started");
        Ok(())
    }

    #[test]
    fn session_origin_reads_the_latest_event() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let origin = session_origin(&app, AgentId::CLAUDE, &SessionId::try_from("s1")?)?;
        assert_eq!(origin.cwd.as_deref(), Some("/work/acme"));
        assert!(!origin.transcript_exists);
        let unknown = session_origin(&app, AgentId::CLAUDE, &SessionId::try_from("nope")?)?;
        assert_eq!(unknown.cwd, None);
        Ok(())
    }
}
