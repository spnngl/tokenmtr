//! Config → query inputs, and the file locations the config decides.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tokenmtr_config::{
    Config, ConfigDocument, ConfigError, ConfigFormat, Env, GitName, MenubarRing, RawProject,
    TimeZoneSetting, TomlCodec,
};
use tokenmtr_core::{ProfileId, SubscriptionSpec};
use tokenmtr_platform::Paths;
use tokenmtr_platform::fs::replace_atomically;
use tokenmtr_query::{
    BudgetScope, BudgetSpec, ClientName, FxPolicy, MetricsKind, MetricsSpec, QueryContext,
    QueryError, RecapConfig, SinkSpec, WeekStart, load_fx_rates, load_subscriptions,
};
use tokenmtr_store::StoreReader;

use crate::error::AppError;

/// Reads the config at `path`; a file that does not exist means the defaults.
///
/// # Errors
///
/// Any [`ConfigError`] except a missing file.
pub fn load_config(path: &Utf8Path, env: &dyn Env) -> Result<Config, ConfigError> {
    match tokenmtr_config::load_with(path, env) {
        Err(ConfigError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            tokenmtr_config::load_str("", ConfigFormat::Toml(TomlCodec), env)
        }
        other => other,
    }
}

/// The store file: `store.path`, else `usage.db` in the data directory.
#[must_use]
pub fn store_path(paths: &Paths, config: &Config) -> Utf8PathBuf {
    store_path_in(paths, Some(config))
}

/// As [`store_path`]; without a config, the default location.
pub(crate) fn store_path_in(paths: &Paths, config: Option<&Config>) -> Utf8PathBuf {
    config
        .and_then(|config| config.store.path.clone())
        .unwrap_or_else(|| paths.data_dir().join("usage.db"))
}

/// Where backups go: `backup.dir`, else `backups` in the data directory.
#[must_use]
pub fn backup_dir(paths: &Paths, config: &Config) -> Utf8PathBuf {
    backup_dir_in(paths, Some(config))
}

/// As [`backup_dir`]; without a config, the default location.
pub(crate) fn backup_dir_in(paths: &Paths, config: Option<&Config>) -> Utf8PathBuf {
    config
        .and_then(|config| config.backup.dir.clone())
        .unwrap_or_else(|| paths.data_dir().join("backups"))
}

/// The zone that buckets days.
#[must_use]
pub fn display_zone(config: &Config) -> TimeZone {
    match &config.display.timezone {
        TimeZoneSetting::System => TimeZone::system(),
        TimeZoneSetting::Named(zone) => zone.clone(),
    }
}

/// The configured budgets with their scopes.
#[must_use]
pub fn budget_specs(config: &Config) -> Vec<BudgetSpec> {
    config
        .budgets
        .iter()
        .map(|entry| BudgetSpec {
            budget: entry.budget.clone(),
            scope: query_scope(&entry.scope),
        })
        .collect()
}

fn query_scope(scope: &tokenmtr_config::BudgetScope) -> BudgetScope {
    BudgetScope {
        agents: scope.agents.clone(),
        profiles: scope.profiles.clone(),
        projects: scope.projects.clone(),
        clients: scope.clients.as_ref().map(|clients| {
            clients
                .iter()
                .filter_map(|client| ClientName::try_from(client.as_str().to_owned()).ok())
                .collect()
        }),
    }
}

/// The configured sinks, as the health queries see them. Config has no per-sink switch, so
/// every sink is enabled.
#[must_use]
pub fn sink_specs(config: &Config) -> Vec<SinkSpec> {
    config
        .sinks
        .iter()
        .map(|sink| SinkSpec {
            id: sink.id.clone(),
            interval: sink.interval,
            datasets: sink.datasets.clone(),
            enabled: true,
        })
        .collect()
}

/// The configured metrics outputs, exporters first, as the health queries see them.
#[must_use]
pub fn metrics_specs(config: &Config) -> Vec<MetricsSpec> {
    let exporters = config.metrics.exporters.iter().map(|e| MetricsSpec {
        id: e.id.clone(),
        kind: MetricsKind::Exporter,
    });
    let remote_write = config.metrics.remote_write.iter().map(|w| MetricsSpec {
        id: w.id.clone(),
        kind: MetricsKind::RemoteWrite,
    });
    exporters.chain(remote_write).collect()
}

/// Everything the recap and status queries take from the config.
#[must_use]
pub fn recap_config(config: &Config) -> RecapConfig {
    RecapConfig {
        budgets: budget_specs(config),
        sinks: sink_specs(config),
        metrics: metrics_specs(config),
        live_window: config.menubar.live_window,
        daemon_stale_after: config.alerts.daemon_stale_after,
    }
}

/// The query context at `now`: the display zone, the config's currency (or `currency` when
/// given), the FX policy (`fx.fixed_eur_per_usd` beats the stored ECB rates) and the
/// subscriptions the store holds.
///
/// # Errors
///
/// [`QueryError`] when the rates or subscriptions cannot be read.
pub fn query_context(
    config: &Config,
    reader: &StoreReader,
    now: Timestamp,
    currency: Option<tokenmtr_core::Currency>,
) -> Result<QueryContext, QueryError> {
    let fx = match config.fx.fixed_eur_per_usd {
        Some(rate) => FxPolicy::fixed(rate),
        None => FxPolicy::from_rates(&load_fx_rates(reader)?),
    };
    Ok(QueryContext::new(
        display_zone(config),
        WeekStart::Monday,
        currency.unwrap_or(config.display.currency),
        fx,
        now,
    )
    .with_subscriptions(load_subscriptions(reader)?))
}

/// Adds a `[[subscriptions]]` entry to the config file at `path`, keeping its comments,
/// and returns the validated config. A file that does not exist yet starts empty.
///
/// # Errors
///
/// [`AppError::Config`] when the result is invalid (for example an overlap); the file is
/// untouched then.
pub fn add_subscription(
    path: &Utf8Path,
    spec: &SubscriptionSpec,
    env: &dyn Env,
) -> Result<Config, AppError> {
    edit_config(path, |document| document.add_subscription(spec, env))
}

/// Adds a `[[projects]]` rule to the config file at `path`, keeping its comments.
///
/// # Errors
///
/// [`AppError::Config`] when the result is invalid; the file is untouched then.
pub fn add_project_rule(
    path: &Utf8Path,
    rule: &RawProject,
    env: &dyn Env,
) -> Result<Config, AppError> {
    edit_config(path, |document| document.add_project_rule(rule, env))
}

/// Sets `[menubar] rings` and `hidden_profiles` in the config file at `path`, keeping its
/// comments.
///
/// # Errors
///
/// [`AppError::Config`] when the result is invalid; the file is untouched then.
pub fn set_menubar_rings(
    path: &Utf8Path,
    rings: &[MenubarRing],
    hidden_profiles: &[ProfileId],
    env: &dyn Env,
) -> Result<Config, AppError> {
    edit_config(path, |document| {
        document.set_menubar_rings(rings, hidden_profiles, env)
    })
}

/// Sets `[git] default_remote` and `default_branch` (removed when `None`) in the config file
/// at `path`, keeping its comments.
///
/// # Errors
///
/// [`AppError::Config`] when the result is invalid; the file is untouched then.
pub fn set_git_defaults(
    path: &Utf8Path,
    default_remote: &GitName,
    default_branch: Option<&GitName>,
    env: &dyn Env,
) -> Result<Config, AppError> {
    edit_config(path, |document| {
        document.set_git_defaults(default_remote, default_branch, env)
    })
}

/// Creates the config file at `path` holding only comments (so the defaults) when it does
/// not exist, so there is always a file to open and edit. Returns whether it created one.
///
/// # Errors
///
/// [`AppError::Io`] when the file or its folder cannot be written.
pub fn ensure_config_file(path: &Utf8Path) -> Result<bool, AppError> {
    if path.exists() {
        return Ok(false);
    }
    let starter = format!(
        "# {name} configuration. Every key is optional: this file holds only what differs from\n\
         # the defaults, and an empty file means the defaults. Check it with `{cli} config check`.\n\
         # docs/config.md in the {name} repository lists every key with an example.\n",
        name = tokenmtr_core::brand::DISPLAY_NAME,
        cli = tokenmtr_core::brand::CLI_NAME,
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    }
    replace_atomically(path, starter.as_bytes()).map_err(|error| AppError::io(path, error))?;
    Ok(true)
}

fn edit_config(
    path: &Utf8Path,
    edit: impl FnOnce(&mut ConfigDocument) -> Result<Config, ConfigError>,
) -> Result<Config, AppError> {
    let mut document = match ConfigDocument::open(path) {
        Err(ConfigError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            ConfigDocument::new("", ConfigFormat::Toml(TomlCodec))
        }
        other => other?,
    };
    let config = edit(&mut document)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    }
    replace_atomically(path, document.text().as_bytes())
        .map_err(|error| AppError::io(path, error))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn config(text: &str) -> Result<Config, ConfigError> {
        tokenmtr_config::load_str(
            text,
            ConfigFormat::Toml(TomlCodec),
            &|_: &str| None::<String>,
        )
    }

    #[test]
    fn missing_config_file_means_the_defaults() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = Utf8Path::from_path(dir.path())
            .ok_or("temp dir is not UTF-8")?
            .join("absent.toml");
        let loaded = load_config(&path, &|_: &str| None::<String>)?;
        assert_eq!(loaded.menubar.agents, config("")?.menubar.agents);
        Ok(())
    }

    #[test]
    fn a_starter_config_is_comments_only_and_never_overwrites() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = Utf8Path::from_path(dir.path())
            .ok_or("temp dir is not UTF-8")?
            .join("nested/config.toml");
        assert!(ensure_config_file(&path)?);
        let text = std::fs::read_to_string(&path)?;
        assert!(text.lines().all(|line| line.starts_with('#')), "{text}");
        load_config(&path, &|_: &str| None::<String>)?;
        std::fs::write(&path, "[quota]\nenabled = false\n")?;
        assert!(!ensure_config_file(&path)?);
        assert!(std::fs::read_to_string(&path)?.contains("[quota]"));
        Ok(())
    }

    #[test]
    fn sinks_become_enabled_specs() -> TestResult {
        let loaded = config(
            r#"
[[sinks]]
id = "files"
kind = "csv"
interval = "10m"
datasets = ["usage_events", "fx_rates"]
"#,
        )?;
        let specs = sink_specs(&loaded);
        assert_eq!(specs.len(), 1);
        assert!(specs[0].enabled);
        assert_eq!(specs[0].datasets.len(), 2);
        Ok(())
    }
}
