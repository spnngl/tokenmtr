//! [`AppContext`]: the config, the file locations and a read-only store, with the queries and
//! actions the CLI and the desktop shell share.

use std::sync::Arc;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use jiff::civil::Date;
use serde::Serialize;
use tokenmtr_agents::ingest::{
    DoctorReport, IngestSettings, Ingestor, ScanPriority, ScanReport, ScanRequest,
};
use tokenmtr_agents::{AgentKind, AgentSource, claude};
use tokenmtr_config::{Config, Env, GitName, MenubarRing, RawProject, SourceTarget};
use tokenmtr_core::{AgentId, Plan, ProfileId, SourceId, SubscriptionSpec};
use tokenmtr_export::Target;
use tokenmtr_import::{ImportOptions, ImportReport, ImportSpec, import_pending};
use tokenmtr_platform::{ControlTransport, Paths};
use tokenmtr_query::{
    AlertView, BackupView, BudgetSpec, DateRange, Filters, FxRateView, Period, QueryContext,
    QuotaMeter, Recap, RecapConfig, Selection, SinkSpec, Status,
};
use tokenmtr_store::{BackupRun, ImportedRows, RepriceScope, Store, StoreRead, StoreReader};

use crate::backup::{BackupFile, backup_path, list_backups, prune_backups};
use crate::error::AppError;
use crate::guard::{daemon_running, lock_daemon};
use crate::report::{RenderedReport, ReportRequest, generate_report};
use crate::retention::set_claude_retention;
use crate::settings::{
    add_project_rule, add_subscription, backup_dir, budget_specs, load_config, query_context,
    recap_config, set_git_defaults, set_menubar_rings, sink_specs, store_path,
};

/// The detected plan of one account (from the provider's account endpoint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountSummary {
    /// The agent.
    pub agent: AgentId,
    /// The profile.
    pub profile: ProfileId,
    /// The plan the provider reports, when it maps to a known one.
    pub detected_plan: Option<Plan>,
    /// The provider's rate-limit tier, verbatim.
    pub rate_limit_tier: Option<String>,
    /// When it was fetched.
    pub fetched_at: Timestamp,
}

/// The latest quota windows and the detected plans.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaReport {
    /// One meter per polled profile.
    pub meters: Vec<QuotaMeter>,
    /// The accounts behind them.
    pub accounts: Vec<AccountSummary>,
}

/// The latest FX rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FxSummary {
    /// `fx.fixed_eur_per_usd` (EUR per USD × 10⁹), which overrides the stored rates.
    pub fixed_eur_per_usd_scaled: Option<i64>,
    /// The newest stored ECB rate.
    pub latest: Option<FxRateView>,
}

/// The active pricing snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PricingSnapshotInfo {
    /// First 12 hex characters of the body's sha256.
    pub version: String,
    /// When it was fetched.
    pub fetched_at: Timestamp,
    /// Where it came from; absent for the embedded snapshot.
    pub source: Option<String>,
    /// Size of the body.
    pub bytes: usize,
}

/// A model with events the catalog cannot price.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnpricedModel {
    /// The model id as reported.
    pub model: String,
    /// Events without a price.
    pub unpriced_events: u64,
}

/// The pricing state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PricingSummary {
    /// The newest stored snapshot.
    pub snapshot: Option<PricingSnapshotInfo>,
    /// Models with unpriced events, most events first.
    pub unpriced_models: Vec<UnpricedModel>,
}

/// What one import run did for one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceImport {
    /// The source.
    pub source: SourceId,
    /// Files read.
    pub files_read: u64,
    /// Rows written (events and tool calls).
    pub rows: u64,
    /// The first failure of the run, when a file or folder failed.
    pub error: Option<String>,
}

impl From<&ImportReport> for SourceImport {
    fn from(report: &ImportReport) -> Self {
        Self {
            source: report.source.clone(),
            files_read: report.read,
            rows: report.rows(),
            error: report
                .failures
                .first()
                .map(|failure| failure.error.to_string()),
        }
    }
}

/// The loaded config, the file locations and a read-only store.
///
/// Queries take `&self`. Actions that write the store (`scan_direct`, `backup_direct`) refuse
/// while a daemon runs, and hold the daemon lock so none starts meanwhile: it is the only
/// writer.
pub struct AppContext {
    paths: Paths,
    transport: ControlTransport,
    config_path: Utf8PathBuf,
    env: Arc<dyn Env + Send + Sync>,
    config: Config,
    store_path: Utf8PathBuf,
    reader: StoreReader,
}

impl AppContext {
    /// Loads the config at `config_path` (missing means the defaults) and opens the store
    /// read-only. A store that does not exist yet is created empty first: a missing file means
    /// no daemon ever ran.
    ///
    /// # Errors
    ///
    /// [`AppError::Config`] for an invalid config; [`AppError::Store`] when the store cannot be
    /// opened, including a newer schema or an older one no daemon has migrated yet.
    pub fn load(
        paths: Paths,
        transport: ControlTransport,
        config_path: Utf8PathBuf,
        env: Arc<dyn Env + Send + Sync>,
    ) -> Result<Self, AppError> {
        let config = load_config(&config_path, &*env)?;
        let store_path = store_path(&paths, &config);
        if !store_path.exists() {
            drop(Store::open(&store_path)?);
        }
        let reader = StoreReader::open(&store_path)?;
        Ok(Self {
            paths,
            transport,
            config_path,
            env,
            config,
            store_path,
            reader,
        })
    }

    /// The default locations.
    #[must_use]
    pub const fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The config in force.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// The config file (it may not exist).
    #[must_use]
    pub fn config_path(&self) -> &Utf8Path {
        &self.config_path
    }

    /// The store file.
    #[must_use]
    pub fn store_path(&self) -> &Utf8Path {
        &self.store_path
    }

    /// The read-only store.
    #[must_use]
    pub const fn reader(&self) -> &StoreReader {
        &self.reader
    }

    /// Where the daemon's control socket is.
    #[must_use]
    pub const fn transport(&self) -> &ControlTransport {
        &self.transport
    }

    /// Whether a daemon answers on the control socket.
    #[must_use]
    pub fn daemon_running(&self) -> bool {
        daemon_running(&self.transport)
    }

    /// Reads the config file again.
    ///
    /// # Errors
    ///
    /// [`AppError::Config`]; the config in force is kept then.
    pub fn reload_config(&mut self) -> Result<(), AppError> {
        self.config = load_config(&self.config_path, &*self.env)?;
        Ok(())
    }

    /// The query context at `now`.
    ///
    /// # Errors
    ///
    /// [`AppError::Query`] when the FX rates or subscriptions cannot be read.
    pub fn query_context(&self, now: Timestamp) -> Result<QueryContext, AppError> {
        Ok(query_context(&self.config, &self.reader, now, None)?)
    }

    /// The configured budgets.
    #[must_use]
    pub fn budget_specs(&self) -> Vec<BudgetSpec> {
        budget_specs(&self.config)
    }

    /// The configured sinks.
    #[must_use]
    pub fn sink_specs(&self) -> Vec<SinkSpec> {
        sink_specs(&self.config)
    }

    /// What the recap queries take from the config.
    #[must_use]
    pub fn recap_config(&self) -> RecapConfig {
        recap_config(&self.config)
    }

    /// The `status --json` document: system health and one recap per followed agent
    /// (`menubar.agents`).
    ///
    /// # Errors
    ///
    /// [`AppError::Query`].
    pub fn status(&self, now: Timestamp, period: Period) -> Result<Status, AppError> {
        Ok(tokenmtr_query::status(
            &self.reader,
            &self.query_context(now)?,
            &self.recap_config(),
            &self.config.menubar.agents,
            period,
        )?)
    }

    /// The recap of one tab.
    ///
    /// # Errors
    ///
    /// [`AppError::Query`].
    pub fn recap(
        &self,
        now: Timestamp,
        selection: Selection,
        period: Period,
    ) -> Result<Recap, AppError> {
        Ok(tokenmtr_query::recap(
            &self.reader,
            &self.query_context(now)?,
            &self.recap_config(),
            selection,
            period,
        )?)
    }

    /// Alerts fired at or after `since`, newest first.
    ///
    /// # Errors
    ///
    /// [`AppError::Query`].
    pub fn alerts(&self, since: Timestamp, limit: u32) -> Result<Vec<AlertView>, AppError> {
        Ok(tokenmtr_query::alerts(&self.reader, since, limit)?)
    }

    /// The latest quota windows per profile and the detected plans.
    ///
    /// # Errors
    ///
    /// [`AppError::Query`], [`AppError::Store`].
    pub fn quota(&self, now: Timestamp) -> Result<QuotaReport, AppError> {
        let ctx = self.query_context(now)?;
        let filters = Filters::new(whole_history(&ctx)?);
        let accounts = self
            .reader
            .accounts()?
            .into_iter()
            .map(|account| AccountSummary {
                agent: account.agent,
                profile: account.profile,
                detected_plan: account.detected_plan,
                rate_limit_tier: account.rate_limit_tier,
                fetched_at: account.fetched_at,
            })
            .collect();
        Ok(QuotaReport {
            meters: tokenmtr_query::quota_latest(&self.reader, &ctx, &filters)?,
            accounts,
        })
    }

    /// The newest stored FX rate and the configured override.
    ///
    /// # Errors
    ///
    /// [`AppError::Query`].
    pub fn fx(&self, now: Timestamp) -> Result<FxSummary, AppError> {
        let ctx = self.query_context(now)?;
        let latest = tokenmtr_query::fx_rates(&self.reader, whole_history(&ctx)?)?.pop();
        Ok(FxSummary {
            fixed_eur_per_usd_scaled: self.config.fx.fixed_eur_per_usd.map(i64::from),
            latest,
        })
    }

    /// The active pricing snapshot and the models it cannot price.
    ///
    /// # Errors
    ///
    /// [`AppError::Query`], [`AppError::Store`].
    pub fn pricing(&self, now: Timestamp) -> Result<PricingSummary, AppError> {
        let ctx = self.query_context(now)?;
        let snapshot = self
            .reader
            .active_pricing_snapshot()?
            .map(|snapshot| PricingSnapshotInfo {
                version: snapshot.version.to_string(),
                fetched_at: snapshot.fetched_at,
                source: snapshot.source,
                bytes: snapshot.body.len(),
            });
        let mut unpriced_models: Vec<UnpricedModel> =
            tokenmtr_query::models(&self.reader, &ctx, &Filters::new(whole_history(&ctx)?))?
                .rows
                .into_iter()
                .filter(|row| row.usage.unpriced_events > 0)
                .map(|row| UnpricedModel {
                    model: row.model,
                    unpriced_events: row.usage.unpriced_events,
                })
                .collect();
        unpriced_models.sort_by(|a, b| {
            b.unpriced_events
                .cmp(&a.unpriced_events)
                .then_with(|| a.model.cmp(&b.model))
        });
        Ok(PricingSummary {
            snapshot,
            unpriced_models,
        })
    }

    /// The backup files on disk, newest first.
    #[must_use]
    pub fn backups(&self) -> Vec<BackupFile> {
        list_backups(&backup_dir(&self.paths, &self.config))
    }

    /// The recorded backup attempts, newest first (failures included).
    ///
    /// # Errors
    ///
    /// [`AppError::Query`].
    pub fn backup_history(&self, limit: u32) -> Result<Vec<BackupView>, AppError> {
        Ok(tokenmtr_query::backup_runs(&self.reader, limit)?)
    }

    /// Writes a backup without the daemon, records the attempt and prunes to `backup.keep`.
    ///
    /// # Errors
    ///
    /// [`AppError::DaemonRunning`] (ask the daemon instead); [`AppError::Store`] when the
    /// backup fails (the failed attempt is recorded too).
    pub fn backup_direct(&self, now: Timestamp) -> Result<BackupFile, AppError> {
        let _lock = lock_daemon(&self.transport, Duration::ZERO)?;
        let dir = backup_dir(&self.paths, &self.config);
        let dest = backup_path(&dir, now);
        let result = StoreReader::open(&self.store_path).and_then(|reader| reader.backup(&dest));
        let finished = Timestamp::now();
        let run = match &result {
            Ok(info) => BackupRun {
                started: now,
                finished,
                path: Some(dest.clone()),
                size_bytes: Some(info.compressed_bytes),
                error: None,
            },
            Err(error) => BackupRun {
                started: now,
                finished,
                path: None,
                size_bytes: None,
                error: Some(error.to_string()),
            },
        };
        Store::open(&self.store_path)?.record_backup_run(&run)?;
        let info = result?;
        // A file that outlives its pruning is only clutter: the backup itself succeeded.
        drop(prune_backups(&dir, self.config.backup.keep));
        Ok(BackupFile {
            path: dest,
            size_bytes: info.compressed_bytes,
        })
    }

    /// Builds and renders a client report.
    ///
    /// # Errors
    ///
    /// As [`generate_report`].
    pub fn generate_report(
        &self,
        now: Timestamp,
        request: &ReportRequest,
    ) -> Result<RenderedReport, AppError> {
        generate_report(&self.config, &self.reader, now, request)
    }

    /// Generates a report and writes it to `path`.
    ///
    /// # Errors
    ///
    /// As [`generate_report`], plus [`AppError::Io`].
    pub fn write_report(
        &self,
        now: Timestamp,
        request: &ReportRequest,
        path: &Utf8Path,
    ) -> Result<RenderedReport, AppError> {
        let report = self.generate_report(now, request)?;
        report.write_to(path)?;
        Ok(report)
    }

    /// Adds a `[[subscriptions]]` entry to the config file, keeping its comments.
    ///
    /// # Errors
    ///
    /// [`AppError::Config`] when the result is invalid (for example an overlap); the file is
    /// untouched then.
    pub fn add_subscription(&mut self, spec: &SubscriptionSpec) -> Result<(), AppError> {
        self.config = add_subscription(&self.config_path, spec, &*self.env)?;
        Ok(())
    }

    /// Adds a `[[projects]]` rule to the config file, keeping its comments.
    ///
    /// # Errors
    ///
    /// [`AppError::Config`] when the result is invalid; the file is untouched then.
    pub fn add_project_rule(&mut self, rule: &RawProject) -> Result<(), AppError> {
        self.config = add_project_rule(&self.config_path, rule, &*self.env)?;
        Ok(())
    }

    /// Sets which quota windows the menu bar draws as rings and which profiles it skips,
    /// keeping the config file's comments.
    ///
    /// # Errors
    ///
    /// [`AppError::Config`] when the result is invalid; the file is untouched then.
    pub fn set_menubar_rings(
        &mut self,
        rings: &[MenubarRing],
        hidden_profiles: &[ProfileId],
    ) -> Result<(), AppError> {
        self.config = set_menubar_rings(&self.config_path, rings, hidden_profiles, &*self.env)?;
        Ok(())
    }

    /// Sets the remote that keys projects and the default branch, keeping the config file's
    /// comments. Projects move to the new remote's key on the next scan.
    ///
    /// # Errors
    ///
    /// [`AppError::Config`] when the result is invalid; the file is untouched then.
    pub fn set_git_defaults(
        &mut self,
        default_remote: &GitName,
        default_branch: Option<&GitName>,
    ) -> Result<(), AppError> {
        self.config = set_git_defaults(
            &self.config_path,
            default_remote,
            default_branch,
            &*self.env,
        )?;
        Ok(())
    }

    /// The name a new `[[projects]]` rule for `cwd` suggests: its repository's when git or
    /// Superset knows it, else its folder's ([`IngestSettings::suggested_project_name`]).
    #[must_use]
    pub fn suggested_project_name(&self, cwd: &Utf8Path, now: Timestamp) -> Option<String> {
        self.ingest_settings().suggested_project_name(cwd, now)
    }

    fn ingest_settings(&self) -> IngestSettings {
        IngestSettings::from_config(&self.config, &self.paths, &*self.env)
    }

    /// Sets `cleanupPeriodDays = 3650` in the `settings.json` of a Claude profile.
    ///
    /// # Errors
    ///
    /// [`AppError::InvalidRequest`] for a profile that is not a configured Claude profile;
    /// otherwise as [`set_claude_retention`].
    pub fn set_claude_retention(&self, profile: &ProfileId) -> Result<(), AppError> {
        let settings = self.ingest_settings();
        let dir = settings
            .sources
            .iter()
            .filter_map(|source| match source {
                AgentKind::Claude(claude) => Some(claude.profiles()),
                AgentKind::Pi(_) => None,
            })
            .flatten()
            .find(|candidate| candidate.id == *profile)
            .and_then(|found| claude::config_dirs(found).next())
            .ok_or_else(|| {
                AppError::InvalidRequest(format!("no Claude profile named {profile}"))
            })?;
        set_claude_retention(dir)
    }

    /// Scans the agents' files into the store without the daemon.
    ///
    /// # Errors
    ///
    /// [`AppError::DaemonRunning`] (ask the daemon instead); store and ingest errors.
    pub fn scan_direct(&self, agent: Option<AgentId>, full: bool) -> Result<ScanReport, AppError> {
        let _lock = lock_daemon(&self.transport, Duration::ZERO)?;
        let mut store = Store::open(&self.store_path)?;
        let mut ingestor = Ingestor::open(self.ingest_settings(), &self.store_path)?;
        let priority = ScanPriority::UserInitiated;
        let request = if full {
            ScanRequest::full(priority)
        } else {
            ScanRequest::incremental(priority)
        };
        let request = match agent {
            Some(agent) => request.for_agent(agent),
            None => request,
        };
        Ok(ingestor.scan(&mut store, &request)?)
    }

    /// Re-estimates stored events with the active pricing snapshot while no daemon runs:
    /// the unpriced ones, or every one with [`RepriceScope::All`]. Returns how many changed.
    ///
    /// # Errors
    ///
    /// [`AppError::DaemonRunning`] while a daemon holds the lock; store or ingest failures.
    pub fn reprice_direct(&self, scope: RepriceScope) -> Result<u64, AppError> {
        let _lock = lock_daemon(&self.transport, Duration::ZERO)?;
        let mut store = Store::open(&self.store_path)?;
        let mut ingestor = Ingestor::open(self.ingest_settings(), &self.store_path)?;
        Ok(ingestor.reprice(&mut store, scope)?)
    }

    /// Imports the enabled `sqlite` sources (or only `only`) while no daemon runs.
    ///
    /// # Errors
    ///
    /// [`AppError::DaemonRunning`] (ask the daemon instead); [`AppError::InvalidRequest`] when
    /// `only` is not an enabled `sqlite` source; store failures. A file that cannot be read is
    /// in its report, not an error.
    pub fn import_direct(&self, only: Option<&SourceId>) -> Result<Vec<SourceImport>, AppError> {
        let specs: Vec<ImportSpec> = self
            .config
            .sources
            .iter()
            .filter(|source| source.enabled)
            .filter(|source| only.is_none_or(|id| *id == source.id))
            .filter_map(|source| ImportSpec::from_config(source, self.config.privacy.redact_paths))
            .collect();
        if let (Some(id), true) = (only, specs.is_empty()) {
            return Err(AppError::InvalidRequest(format!(
                "no enabled sqlite source named `{id}`"
            )));
        }
        let _lock = lock_daemon(&self.transport, Duration::ZERO)?;
        let mut store = Store::open(&self.store_path)?;
        Ok(specs
            .iter()
            .map(|spec| import_pending(&mut store, spec, &ImportOptions::default(), &|| false))
            .map(|report| SourceImport::from(&report))
            .collect())
    }

    /// Deletes the rows imported for `source` and the cursors that tracked them. The files are
    /// untouched, so enabling the source again imports everything again.
    ///
    /// # Errors
    ///
    /// [`AppError::InvalidRequest`] while the config still enables the source (its next import
    /// would bring everything back); [`AppError::DaemonRunning`]; store failures.
    pub fn forget_source(&self, source: &SourceId) -> Result<ImportedRows, AppError> {
        let enabled = self.config.sources.iter().any(|candidate| {
            candidate.id == *source
                && candidate.enabled
                && matches!(candidate.target, SourceTarget::Sqlite(_))
        });
        if enabled {
            return Err(AppError::InvalidRequest(format!(
                "source `{source}` is still enabled: disable or remove it in the config first"
            )));
        }
        let _lock = lock_daemon(&self.transport, Duration::ZERO)?;
        let mut store = Store::open(&self.store_path)?;
        Ok(store.forget_source(source)?)
    }

    /// What `doctor` reports about agent files, checkpoints, issues and retention.
    ///
    /// # Errors
    ///
    /// [`AppError::Ingest`].
    pub fn ingest_report(&self) -> Result<DoctorReport, AppError> {
        let ingestor = Ingestor::open(self.ingest_settings(), &self.store_path)?;
        Ok(ingestor.doctor(&self.reader, &external_dirs(&self.paths, &self.config))?)
    }
}

/// The folders outside the agents' roots the config points at, checked against protected
/// folders: the directories of the configured sinks, and the folders the enabled `sqlite`
/// sources read.
#[must_use]
pub fn external_dirs(paths: &Paths, config: &Config) -> Vec<Utf8PathBuf> {
    let sinks =
        config.sinks.iter().filter_map(|sink| {
            match tokenmtr_export::SinkSpec::from_config(sink, &paths.exports_dir()).target {
                Target::File(file) => Some(file.dir),
                Target::Sqlite(sqlite) => sqlite.path.parent().map(Utf8Path::to_owned),
            }
        });
    let sources = config
        .sources
        .iter()
        .filter(|source| source.enabled)
        .flat_map(|source| match &source.target {
            SourceTarget::Sqlite(sqlite) => tokenmtr_import::include_roots(&sqlite.include),
            SourceTarget::Superset(_) => Vec::new(),
        });
    sinks.chain(sources).collect()
}

/// Every day the store can hold: from 2000 to tomorrow.
fn whole_history(ctx: &QueryContext) -> Result<DateRange, AppError> {
    let end = ctx
        .today()
        .tomorrow()
        .map_err(|error| AppError::InvalidRequest(error.to_string()))?;
    Ok(DateRange::new(Date::constant(2000, 1, 1), end)?)
}
