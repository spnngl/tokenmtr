//! Validation: [`RawConfig`] into [`Config`], applying defaults and the cross-field rules.
//!
//! Checks run in a fixed order and stop at the first violation: schema version, agents,
//! then the remaining tables in the order of `docs/config.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use camino::Utf8PathBuf;
use jiff::SignedDuration;
use jiff::tz::TimeZone;
use tokenmtr_core::{
    AgentId, AlertConfig, Budget, BudgetMeasure, Currency, EurMicros, EurPerUsd, Money, Override,
    PlanCatalog, ProfileId, Subscription, SubscriptionSpec, UsdMicros,
};

use crate::env::Env;
use crate::error::{ConfigPath, Rejection, Violation};
use crate::model::{
    AccountLabels, AccountName, AdvisorConfig, AgentConfig, AlertSettings, BackupConfig,
    BudgetEntry, BudgetScope, CONFIG_SCHEMA_VERSION, Config, DEFAULT_HEADROOM, DaemonConfig,
    Dataset, DisplayConfig, FileFormat, FileSink, FxConfig, GitConfig, GitName, ImportConfig,
    IntegrationsConfig, LoggingConfig, MAX_MACHINE_NAME_BYTES, MachineConfig, MenubarConfig,
    MenubarRing, Percent, PricingConfig, PrivacyConfig, ProfileConfig, ProfileLocation,
    ProjectRule, QuotaConfig, SinkConfig, SinkTarget, SqliteSink, StoreConfig, SupersetConfig,
    TimeZoneSetting, path_glob,
};
use crate::raw::{
    Amount, RawAgents, RawBudget, RawConfig, RawDuration, RawOverride, RawProject, RawSink,
    RawSinkKind, RawSubscription,
};

mod metrics;

type Checked<T> = Result<T, Rejection>;

const DEFAULT_SHORTCUT: &str = "Alt+CommandOrControl+U";
const MIN_QUOTA_INTERVAL: SignedDuration = SignedDuration::from_mins(1);

fn reject(path: &ConfigPath, violation: impl Into<Violation>) -> Rejection {
    Rejection {
        path: path.clone(),
        violation: violation.into(),
    }
}

impl Config {
    /// Validates `raw`: applies defaults, expands `~` through `env`, and enforces every rule of
    /// `docs/config.md`.
    ///
    /// # Errors
    ///
    /// The first [`Rejection`] found: the offending config path and the rule it breaks.
    pub fn from_raw(raw: &RawConfig, env: &dyn Env) -> Result<Self, Rejection> {
        let root = ConfigPath::root();
        if let Some(found) = raw.schema_version
            && found != CONFIG_SCHEMA_VERSION
        {
            return Err(reject(
                &root.key("schema_version"),
                Violation::UnsupportedSchemaVersion {
                    found,
                    supported: CONFIG_SCHEMA_VERSION,
                },
            ));
        }
        let agents = build_agents(&raw.agents, env)?;
        let enabled: Vec<AgentId> = agents
            .iter()
            .filter(|(_, agent)| agent.enabled)
            .map(|(id, _)| *id)
            .collect();
        let display = build_display(raw)?;
        Ok(Self {
            machine: build_machine(raw, &root)?,
            quota: build_quota(raw)?,
            fx: build_fx(raw)?,
            menubar: build_menubar(raw, &enabled)?,
            alerts: build_alerts(raw),
            advisor: AdvisorConfig {
                headroom: raw.advisor.headroom.unwrap_or(DEFAULT_HEADROOM),
            },
            backup: build_backup(raw, env)?,
            import: ImportConfig {
                claude_stats_cache: raw.import.claude_stats_cache.unwrap_or(true),
            },
            integrations: IntegrationsConfig {
                superset: SupersetConfig {
                    enabled: raw.integrations.superset.enabled.unwrap_or(true),
                    dir: optional_path(
                        env,
                        raw.integrations.superset.dir.as_deref(),
                        &root.key("integrations").key("superset").key("dir"),
                    )?,
                },
            },
            git: GitConfig {
                default_remote: raw
                    .git
                    .default_remote
                    .clone()
                    .unwrap_or_else(GitName::origin),
                default_branch: raw.git.default_branch.clone(),
            },
            projects: build_projects(&raw.projects, env)?,
            budgets: build_budgets(&raw.budgets, display.currency, &agents, &enabled)?,
            pricing: build_pricing(raw)?,
            subscriptions: build_subscriptions(&raw.subscriptions, &agents)?,
            daemon: DaemonConfig {
                scan_interval: duration_or(raw.daemon.scan_interval, SignedDuration::from_mins(5)),
                watch: raw.daemon.watch.unwrap_or(true),
                log_level: raw.daemon.log_level.unwrap_or_default(),
            },
            logging: LoggingConfig {
                format: raw.logging.format.unwrap_or_default(),
            },
            store: StoreConfig {
                path: optional_path(
                    env,
                    raw.store.path.as_deref(),
                    &root.key("store").key("path"),
                )?,
            },
            privacy: PrivacyConfig {
                redact_paths: raw.privacy.redact_paths.unwrap_or(false),
            },
            sinks: build_sinks(&raw.sinks, env)?,
            metrics: metrics::build_metrics(raw, env)?,
            accounts: build_accounts(raw),
            display,
            agents,
        })
    }
}

fn duration_or(value: Option<RawDuration>, default: SignedDuration) -> SignedDuration {
    value.map_or(default, |d| d.0)
}

// -- paths -------------------------------------------------------------------------------------

fn home(env: &dyn Env) -> Option<Utf8PathBuf> {
    env.var("HOME")
        .or_else(|| env.var("USERPROFILE"))
        .map(Utf8PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// Replaces a leading `~` with the home directory.
fn expand_tilde(env: &dyn Env, text: &str, at: &ConfigPath) -> Checked<String> {
    let rest = if text == "~" {
        Some("")
    } else {
        text.strip_prefix("~/")
    };
    let Some(rest) = rest else {
        return Ok(text.to_owned());
    };
    let home = home(env).ok_or_else(|| {
        reject(
            at,
            Violation::HomeUnset {
                path: text.to_owned(),
            },
        )
    })?;
    Ok(if rest.is_empty() {
        home.into_string()
    } else {
        home.join(rest).into_string()
    })
}

fn absolute_path(env: &dyn Env, text: &str, at: &ConfigPath) -> Checked<Utf8PathBuf> {
    let path = Utf8PathBuf::from(expand_tilde(env, text, at)?);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(reject(
            at,
            Violation::RelativePath {
                path: text.to_owned(),
            },
        ))
    }
}

fn optional_path(
    env: &dyn Env,
    text: Option<&str>,
    at: &ConfigPath,
) -> Checked<Option<Utf8PathBuf>> {
    text.map(|text| absolute_path(env, text, at)).transpose()
}

// -- lists -------------------------------------------------------------------------------------

/// Rejects duplicates in `items`, pointing at the second occurrence.
fn unique<T: PartialEq + Display>(items: &[T], at: &ConfigPath) -> Checked<()> {
    for (i, item) in items.iter().enumerate() {
        if items[..i].contains(item) {
            return Err(reject(
                &at.index(i),
                Violation::DuplicateEntry(item.to_string()),
            ));
        }
    }
    Ok(())
}

fn non_empty_unique<T: PartialEq + Display>(items: &[T], at: &ConfigPath) -> Checked<()> {
    if items.is_empty() {
        return Err(reject(at, Violation::EmptyList));
    }
    unique(items, at)
}

fn enabled_agents_only(agents: &[AgentId], enabled: &[AgentId], at: &ConfigPath) -> Checked<()> {
    unique(agents, at)?;
    match agents
        .iter()
        .enumerate()
        .find(|(_, agent)| !enabled.contains(agent))
    {
        Some((i, agent)) => Err(reject(&at.index(i), Violation::AgentDisabled(*agent))),
        None => Ok(()),
    }
}

/// Every profile must be declared by an agent in `scope`.
fn declared_profiles(
    profiles: &[ProfileId],
    scope: &[AgentId],
    agents: &BTreeMap<AgentId, AgentConfig>,
    at: &ConfigPath,
) -> Checked<()> {
    unique(profiles, at)?;
    let declared = |profile: &ProfileId| {
        scope
            .iter()
            .filter_map(|agent| agents.get(agent))
            .any(|agent| agent.profiles.iter().any(|p| &p.id == profile))
    };
    match profiles
        .iter()
        .enumerate()
        .find(|(_, profile)| !declared(profile))
    {
        Some((i, profile)) => Err(reject(
            &at.index(i),
            Violation::UnknownProfile(profile.clone()),
        )),
        None => Ok(()),
    }
}

// -- money -------------------------------------------------------------------------------------

fn money(currency: Currency, amount: Amount, at: &ConfigPath) -> Checked<Money> {
    match currency {
        Currency::Usd => UsdMicros::from_decimal(amount.0).map(Money::Usd),
        Currency::Eur => EurMicros::from_decimal(amount.0).map(Money::Eur),
    }
    .map_err(|e| reject(at, e))
}

// -- tables ------------------------------------------------------------------------------------

/// `[machine]`: a blank `name` is the default; any other is trimmed and must be a printable
/// label value of at most [`MAX_MACHINE_NAME_BYTES`].
fn build_machine(raw: &RawConfig, root: &ConfigPath) -> Checked<MachineConfig> {
    let at = root.key("machine").key("name");
    let name = raw.machine.name.as_deref().map(str::trim);
    let name = match name {
        None | Some("") => None,
        Some(name) if name.chars().any(char::is_control) => {
            return Err(reject(&at, Violation::NotPrintable));
        }
        Some(name) if name.len() > MAX_MACHINE_NAME_BYTES => {
            return Err(reject(
                &at,
                Violation::TooLong {
                    max_bytes: MAX_MACHINE_NAME_BYTES,
                },
            ));
        }
        Some(name) => Some(name.to_owned()),
    };
    Ok(MachineConfig { name })
}

fn build_display(raw: &RawConfig) -> Checked<DisplayConfig> {
    let at = ConfigPath::root().key("display").key("timezone");
    let timezone = match raw.display.timezone.as_deref() {
        None | Some("system") => TimeZoneSetting::System,
        Some(name) => TimeZone::get(name)
            .map(TimeZoneSetting::Named)
            .map_err(|e| {
                reject(
                    &at,
                    Violation::InvalidTimeZone {
                        name: name.to_owned(),
                        reason: e.to_string(),
                    },
                )
            })?,
    };
    Ok(DisplayConfig {
        timezone,
        currency: raw.display.currency.unwrap_or(Currency::Usd),
    })
}

fn build_quota(raw: &RawConfig) -> Checked<QuotaConfig> {
    let interval = duration_or(raw.quota.interval, SignedDuration::from_mins(5));
    if interval < MIN_QUOTA_INTERVAL {
        return Err(reject(
            &ConfigPath::root().key("quota").key("interval"),
            Violation::IntervalTooShort { min: "1m" },
        ));
    }
    Ok(QuotaConfig {
        enabled: raw.quota.enabled.unwrap_or(true),
        interval,
    })
}

fn build_fx(raw: &RawConfig) -> Checked<FxConfig> {
    let fixed_eur_per_usd = raw
        .fx
        .fixed_eur_per_usd
        .map(|rate| EurPerUsd::from_decimal(rate.0))
        .transpose()
        .map_err(|e| reject(&ConfigPath::root().key("fx").key("fixed_eur_per_usd"), e))?;
    Ok(FxConfig {
        enabled: raw.fx.enabled.unwrap_or(true),
        fixed_eur_per_usd,
    })
}

fn build_menubar(raw: &RawConfig, enabled: &[AgentId]) -> Checked<MenubarConfig> {
    let at = ConfigPath::root().key("menubar");
    let agents = match &raw.menubar.agents {
        Some(agents) => {
            let at = at.key("agents");
            non_empty_unique(agents, &at)?;
            enabled_agents_only(agents, enabled, &at)?;
            agents.clone()
        }
        None => enabled.to_vec(),
    };
    let rings = match &raw.menubar.rings {
        Some(rings) => {
            unique(rings, &at.key("rings"))?;
            rings.clone()
        }
        None => vec![MenubarRing::Session, MenubarRing::Weekly],
    };
    let hidden_profiles = raw.menubar.hidden_profiles.clone().unwrap_or_default();
    unique(&hidden_profiles, &at.key("hidden_profiles"))?;
    let shortcut = match raw.menubar.shortcut.as_deref() {
        None => Some(DEFAULT_SHORTCUT.to_owned()),
        Some("") => None,
        Some(shortcut) => Some(shortcut.to_owned()),
    };
    Ok(MenubarConfig {
        agents,
        rings,
        hidden_profiles,
        shortcut,
        live_window: duration_or(raw.menubar.live_window, SignedDuration::from_mins(10)),
    })
}

fn thresholds(raw: Option<&Vec<Percent>>, default: &[u8]) -> Vec<u8> {
    raw.map_or_else(
        || default.to_vec(),
        |list| {
            let sorted: BTreeSet<u8> = list.iter().map(|p| p.get()).collect();
            sorted.into_iter().collect()
        },
    )
}

fn build_alerts(raw: &RawConfig) -> AlertSettings {
    let defaults = AlertConfig::default();
    let alerts = &raw.alerts;
    AlertSettings {
        rules: AlertConfig {
            quota_thresholds: thresholds(
                alerts.quota_thresholds.as_ref(),
                &defaults.quota_thresholds,
            ),
            quota_reset: alerts.quota_reset.unwrap_or(defaults.quota_reset),
            budget_thresholds: thresholds(
                alerts.budget_thresholds.as_ref(),
                &defaults.budget_thresholds,
            ),
            sink_failing_after: duration_or(alerts.sink_failing_after, defaults.sink_failing_after),
        },
        daemon_stale_after: duration_or(alerts.daemon_stale_after, SignedDuration::from_mins(15)),
    }
}

fn build_backup(raw: &RawConfig, env: &dyn Env) -> Checked<BackupConfig> {
    let at = ConfigPath::root().key("backup");
    let keep = raw.backup.keep.unwrap_or(14);
    if keep == 0 {
        return Err(reject(&at.key("keep"), Violation::ZeroKeep));
    }
    Ok(BackupConfig {
        enabled: raw.backup.enabled.unwrap_or(true),
        interval: duration_or(raw.backup.interval, SignedDuration::from_hours(24)),
        keep,
        dir: optional_path(env, raw.backup.dir.as_deref(), &at.key("dir"))?,
    })
}

fn build_projects(raw: &[RawProject], env: &dyn Env) -> Checked<Vec<ProjectRule>> {
    raw.iter()
        .enumerate()
        .map(|(i, rule)| {
            let at = ConfigPath::root().key("projects").index(i).key("match");
            let pattern = expand_tilde(env, &rule.pattern, &at)?;
            let glob = path_glob(&pattern).map_err(|e| invalid_glob(&at, &pattern, &e))?;
            Ok(ProjectRule {
                matcher: glob.compile_matcher(),
                name: rule.name.clone(),
                client: rule.client.clone(),
            })
        })
        .collect()
}

fn invalid_glob(at: &ConfigPath, pattern: &str, error: &globset::Error) -> Rejection {
    reject(
        at,
        Violation::InvalidGlob {
            pattern: error.glob().unwrap_or(pattern).to_owned(),
            reason: error.kind().to_string(),
        },
    )
}

fn build_budgets(
    raw: &[RawBudget],
    display_currency: Currency,
    agents: &BTreeMap<AgentId, AgentConfig>,
    enabled: &[AgentId],
) -> Checked<Vec<BudgetEntry>> {
    let mut entries: Vec<BudgetEntry> = Vec::with_capacity(raw.len());
    for (i, budget) in raw.iter().enumerate() {
        let at = ConfigPath::root().key("budgets").index(i);
        if entries.iter().any(|e| e.budget.id() == &budget.id) {
            return Err(reject(
                &at.key("id"),
                Violation::DuplicateId {
                    kind: "budget",
                    id: budget.id.to_string(),
                },
            ));
        }
        let amount_at = at.key("amount");
        let amount = money(
            budget.currency.unwrap_or(display_currency),
            budget.amount,
            &amount_at,
        )?;
        let core = Budget::new(
            budget.id.clone(),
            budget.period,
            amount,
            budget.measure.unwrap_or(BudgetMeasure::Paid),
        )
        .map_err(|e| reject(&amount_at, e))?;
        entries.push(BudgetEntry {
            budget: core,
            scope: build_scope(budget, &at, agents, enabled)?,
        });
    }
    Ok(entries)
}

fn build_scope(
    budget: &RawBudget,
    at: &ConfigPath,
    agents: &BTreeMap<AgentId, AgentConfig>,
    enabled: &[AgentId],
) -> Checked<BudgetScope> {
    if let Some(list) = &budget.agents {
        let at = at.key("agents");
        non_empty_unique(list, &at)?;
        enabled_agents_only(list, enabled, &at)?;
    }
    if let Some(list) = &budget.profiles {
        let at = at.key("profiles");
        non_empty_unique(list, &at)?;
        let scope = budget.agents.as_deref().unwrap_or(enabled);
        declared_profiles(list, scope, agents, &at)?;
    }
    if let Some(list) = &budget.projects {
        non_empty_unique(list, &at.key("projects"))?;
    }
    if let Some(list) = &budget.clients {
        non_empty_unique(list, &at.key("clients"))?;
    }
    Ok(BudgetScope {
        agents: budget.agents.clone(),
        profiles: budget.profiles.clone(),
        projects: budget.projects.clone(),
        clients: budget.clients.clone(),
    })
}

fn build_pricing(raw: &RawConfig) -> Checked<PricingConfig> {
    let overrides = raw
        .pricing
        .overrides
        .iter()
        .enumerate()
        .map(|(i, o)| {
            build_override(
                o,
                &ConfigPath::root().key("pricing").key("overrides").index(i),
            )
        })
        .collect::<Checked<Vec<_>>>()?;
    Ok(PricingConfig {
        auto_update: raw.pricing.auto_update.unwrap_or(true),
        overrides,
    })
}

fn build_override(raw: &RawOverride, at: &ConfigPath) -> Checked<Override> {
    let built = Override::new(
        raw.model.clone(),
        raw.input_per_m.0,
        raw.output_per_m.0,
        raw.cache_read_per_m.map(|a| a.0),
        raw.cache_write_5m_per_m.map(|a| a.0),
        raw.cache_write_1h_per_m.map(|a| a.0),
    )
    .map_err(|e| reject(at, e))?;
    Ok(match &raw.provider {
        Some(provider) => built.for_provider(provider.clone()),
        None => built,
    })
}

fn build_subscriptions(
    raw: &[RawSubscription],
    agents: &BTreeMap<AgentId, AgentConfig>,
) -> Checked<Vec<Subscription>> {
    let root = ConfigPath::root().key("subscriptions");
    let catalog = if raw.iter().any(|s| s.fee.is_none()) {
        Some(PlanCatalog::embedded().map_err(|e| reject(&root, Box::new(e)))?)
    } else {
        None
    };
    let enabled: Vec<AgentId> = agents
        .iter()
        .filter(|(_, a)| a.enabled)
        .map(|(id, _)| *id)
        .collect();
    let mut built: Vec<Subscription> = Vec::with_capacity(raw.len());
    for (i, sub) in raw.iter().enumerate() {
        let at = root.index(i);
        if built.iter().any(|s| s.id() == &sub.id) {
            return Err(reject(
                &at.key("id"),
                Violation::DuplicateId {
                    kind: "subscription",
                    id: sub.id.to_string(),
                },
            ));
        }
        enabled_agents_only(&sub.agents, &enabled, &at.key("agents"))?;
        if let Some(profiles) = &sub.profiles {
            declared_profiles(profiles, &sub.agents, agents, &at.key("profiles"))?;
        }
        let currency = sub.fee_currency.unwrap_or(Currency::Usd);
        let fee = match sub.fee {
            Some(amount) => money(currency, amount, &at.key("fee"))?,
            None => catalog
                .as_ref()
                .and_then(|c| c.default_fee(sub.plan, sub.period))
                .filter(|fee| fee.currency() == currency)
                .ok_or_else(|| {
                    reject(
                        &at,
                        Violation::FeeRequired {
                            subscription: sub.id.clone(),
                            plan: sub.plan,
                            period: sub.period,
                            currency,
                        },
                    )
                })?,
        };
        let subscription = Subscription::new(SubscriptionSpec {
            id: sub.id.clone(),
            provider: sub.provider.clone(),
            plan: sub.plan,
            agents: sub.agents.clone(),
            profiles: sub.profiles.clone(),
            fee,
            period: sub.period,
            start: sub.start,
            end: sub.end,
        })
        .map_err(|e| reject(&at, e))?;
        if let Some(first) = built.iter().find(|s| s.overlaps(&subscription)) {
            return Err(reject(
                &at,
                Violation::OverlappingSubscriptions {
                    first: first.id().clone(),
                    second: subscription.id().clone(),
                },
            ));
        }
        built.push(subscription);
    }
    Ok(built)
}

// -- agents ------------------------------------------------------------------------------------

/// An agent as written: the one list every per-agent rule reads.
struct AgentSource<'a> {
    id: AgentId,
    enabled: Option<bool>,
    account: Option<&'a AccountName>,
    profiles: Vec<ProfileSource<'a>>,
}

/// A profile as written, whatever the agent.
struct ProfileSource<'a> {
    id: &'a ProfileId,
    roots: Option<&'a [String]>,
    config_dir: Option<&'a str>,
    account: Option<&'a AccountName>,
}

fn agent_sources(raw: &RawAgents) -> [AgentSource<'_>; 2] {
    [
        AgentSource {
            id: AgentId::CLAUDE,
            enabled: raw.claude.enabled,
            account: raw.claude.account.as_ref(),
            profiles: raw
                .claude
                .profiles
                .iter()
                .map(|p| ProfileSource {
                    id: &p.id,
                    roots: p.roots.as_deref(),
                    config_dir: p.config_dir.as_deref(),
                    account: p.account.as_ref(),
                })
                .collect(),
        },
        AgentSource {
            id: AgentId::PI,
            enabled: raw.pi.enabled,
            account: raw.pi.account.as_ref(),
            profiles: raw
                .pi
                .profiles
                .iter()
                .map(|p| ProfileSource {
                    id: &p.id,
                    roots: p.roots.as_deref(),
                    config_dir: None,
                    account: p.account.as_ref(),
                })
                .collect(),
        },
    ]
}

fn build_agents(raw: &RawAgents, env: &dyn Env) -> Checked<BTreeMap<AgentId, AgentConfig>> {
    agent_sources(raw)
        .iter()
        .map(|agent| {
            let built = build_agent(env, agent.id, agent.enabled, &agent.profiles)?;
            Ok((agent.id, built))
        })
        .collect()
}

/// Runs after [`build_agents`], which rejects duplicate profile ids.
fn build_accounts(raw: &RawConfig) -> AccountLabels {
    let sources = agent_sources(&raw.agents);
    AccountLabels {
        global: raw.account.clone(),
        agents: sources
            .iter()
            .filter_map(|agent| Some((agent.id, agent.account?.clone())))
            .collect(),
        profiles: sources
            .iter()
            .flat_map(|agent| {
                agent.profiles.iter().filter_map(|profile| {
                    Some(((agent.id, profile.id.clone()), profile.account?.clone()))
                })
            })
            .collect(),
    }
}

fn build_agent(
    env: &dyn Env,
    agent: AgentId,
    enabled: Option<bool>,
    sources: &[ProfileSource<'_>],
) -> Checked<AgentConfig> {
    let base = ConfigPath::root()
        .key("agents")
        .key(agent.as_str())
        .key("profiles");
    let mut profiles: Vec<ProfileConfig> = Vec::with_capacity(sources.len() + 1);
    // Explicit paths seen so far, with the profile that declared them.
    let mut claimed: Vec<(ProfileId, Utf8PathBuf)> = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        let at = base.index(i);
        if profiles.iter().any(|p| &p.id == source.id) {
            return Err(reject(
                &at.key("id"),
                Violation::DuplicateId {
                    kind: "profile",
                    id: source.id.to_string(),
                },
            ));
        }
        let (location, paths, paths_at) = match (source.roots, source.config_dir) {
            (Some(_), Some(_)) => {
                return Err(reject(&at, Violation::RootsAndConfigDir(source.id.clone())));
            }
            (Some(roots), None) => {
                let at = at.key("roots");
                non_empty_unique(roots, &at)?;
                let paths = roots
                    .iter()
                    .enumerate()
                    .map(|(j, root)| absolute_path(env, root, &at.index(j)))
                    .collect::<Checked<Vec<_>>>()?;
                (ProfileLocation::Roots(paths.clone()), paths, at)
            }
            (None, Some(dir)) => {
                let at = at.key("config_dir");
                let path = absolute_path(env, dir, &at)?;
                (ProfileLocation::ConfigDir(path.clone()), vec![path], at)
            }
            (None, None) => (ProfileLocation::Default, Vec::new(), at),
        };
        for path in paths {
            if let Some((other, _)) = claimed.iter().find(|(owner, claimed)| {
                owner != source.id && (path.starts_with(claimed) || claimed.starts_with(&path))
            }) {
                return Err(reject(
                    &paths_at,
                    Violation::SharedRoot {
                        agent,
                        profile: source.id.clone(),
                        other: other.clone(),
                        path,
                    },
                ));
            }
            claimed.push((source.id.clone(), path));
        }
        profiles.push(ProfileConfig {
            id: source.id.clone(),
            location,
        });
    }
    let default = ProfileId::default_profile();
    if !profiles.iter().any(|p| p.id == default) {
        profiles.insert(
            0,
            ProfileConfig {
                id: default,
                location: ProfileLocation::Default,
            },
        );
    }
    Ok(AgentConfig {
        enabled: enabled.unwrap_or(true),
        profiles,
    })
}

// -- sinks -------------------------------------------------------------------------------------

fn build_sinks(raw: &[RawSink], env: &dyn Env) -> Checked<Vec<SinkConfig>> {
    let mut sinks: Vec<SinkConfig> = Vec::with_capacity(raw.len());
    for (i, sink) in raw.iter().enumerate() {
        let at = ConfigPath::root().key("sinks").index(i);
        if sinks.iter().any(|s| s.id == sink.id) {
            return Err(reject(
                &at.key("id"),
                Violation::DuplicateSinkId(sink.id.clone()),
            ));
        }
        let datasets = match &sink.datasets {
            Some(list) => {
                non_empty_unique(list, &at.key("datasets"))?;
                list.clone()
            }
            None => Dataset::ALL.to_vec(),
        };
        let built = SinkConfig {
            id: sink.id.clone(),
            interval: duration_or(sink.interval, SignedDuration::from_mins(5)),
            datasets,
            redact: sink.redact.clone().unwrap_or_default(),
            target: build_target(sink, &at, env)?,
        };
        if let Some(other) = sinks.iter().find(|s| same_files(s, &built)) {
            return Err(reject(
                &at.key("kind"),
                Violation::SinkTargetClash(other.id.clone()),
            ));
        }
        sinks.push(built);
    }
    Ok(sinks)
}

/// Whether two sinks would write the same files: same location and a dataset in common. An unset
/// `dir` or `path` is the exports directory, which the config does not know, so a path spelled
/// out to that directory is not compared with an unset one.
fn same_files(a: &SinkConfig, b: &SinkConfig) -> bool {
    let same_place = match (&a.target, &b.target) {
        (SinkTarget::File(x), SinkTarget::File(y)) => x.format == y.format && x.dir == y.dir,
        (SinkTarget::Sqlite(x), SinkTarget::Sqlite(y)) => x.path == y.path,
        _ => false,
    };
    same_place && a.datasets.iter().any(|d| b.datasets.contains(d))
}

fn build_target(sink: &RawSink, at: &ConfigPath, env: &dyn Env) -> Checked<SinkTarget> {
    let not_allowed = |kind: &'static str, field: &'static str| {
        reject(&at.key(field), Violation::FieldNotAllowed { kind, field })
    };
    match sink.kind {
        RawSinkKind::Csv | RawSinkKind::Jsonl => {
            let (kind, format) = if sink.kind == RawSinkKind::Csv {
                ("csv", FileFormat::Csv)
            } else {
                ("jsonl", FileFormat::Jsonl)
            };
            if sink.path.is_some() {
                return Err(not_allowed(kind, "path"));
            }
            Ok(SinkTarget::File(FileSink {
                format,
                dir: optional_path(env, sink.dir.as_deref(), &at.key("dir"))?,
                mode: sink.mode.unwrap_or_default(),
                partition: sink.partition.unwrap_or_default(),
            }))
        }
        RawSinkKind::Sqlite => {
            if sink.dir.is_some() {
                return Err(not_allowed("sqlite", "dir"));
            }
            if sink.mode.is_some() {
                return Err(not_allowed("sqlite", "mode"));
            }
            if sink.partition.is_some() {
                return Err(not_allowed("sqlite", "partition"));
            }
            Ok(SinkTarget::Sqlite(SqliteSink {
                path: optional_path(env, sink.path.as_deref(), &at.key("path"))?,
            }))
        }
    }
}
