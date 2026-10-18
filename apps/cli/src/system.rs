//! `tokenmtr doctor` and `tokenmtr config`.

use anyhow::{Context, bail};
use clap::{Args, Subcommand, ValueEnum};
use jiff::civil::Date;
use rust_decimal::Decimal;
use tokenmtr_app::{Diagnosis, diagnose};
use tokenmtr_config::{ClientName, ProcessEnv, RawProject};
use tokenmtr_core::{
    AgentId, BillingPeriod, EurMicros, Money, Plan, PlanCatalog, ProfileId, ProjectKey, ProviderId,
    SubscriptionId, SubscriptionSpec, UsdMicros,
};
use tokenmtr_platform::Platform;

use crate::format::{bytes, print_json, timestamp};
use crate::setup::Setup;

/// Arguments of `tokenmtr doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

/// `tokenmtr doctor`: what is wrong, and what is not. Fails (exit 1) after printing the report
/// when the config or the store is broken, so scripts can rely on the exit code.
pub fn doctor(setup: &Setup, args: &DoctorArgs) -> anyhow::Result<()> {
    let diagnosis = diagnose(
        setup.paths(),
        &setup.transport(),
        setup.config_path(),
        &ProcessEnv,
        setup.platform().service(),
    );
    if args.json {
        print_json(&diagnosis)?;
    } else {
        render_diagnosis(&diagnosis);
    }
    match (&diagnosis.config_error, &diagnosis.store_error) {
        (None, None) => Ok(()),
        (Some(_), _) => bail!("the config is invalid"),
        (None, Some(_)) => bail!("the store cannot be read"),
    }
}

fn render_diagnosis(diagnosis: &Diagnosis) {
    println!("config: {}", diagnosis.config_path);
    match &diagnosis.config_error {
        Some(error) => println!("  invalid: {error}"),
        None => println!("  valid"),
    }
    if let Some(store) = &diagnosis.store_path {
        println!("store: {store}");
    }
    match (&diagnosis.store_error, diagnosis.store_schema) {
        (Some(error), _) => println!("  problem: {error}"),
        (None, Some(schema)) => println!(
            "  schema {schema} (this build supports {})",
            diagnosis.supported_schema
        ),
        (None, None) => {}
    }
    println!(
        "daemon: {}",
        if diagnosis.daemon_running {
            "answering on the control socket"
        } else {
            "not running"
        }
    );
    if let Some(service) = &diagnosis.service {
        println!("service: {service}");
    }
    if let Some(ingest) = &diagnosis.ingest {
        for agent in &ingest.agents {
            println!(
                "agent {}: {} checkpointed files",
                agent.agent, agent.checkpoints
            );
            for profile in &agent.profiles {
                for root in &profile.roots {
                    let state = if root.exists {
                        format!("{} files, {}", root.files, bytes(root.bytes))
                    } else {
                        "missing".to_owned()
                    };
                    println!("  {} {}: {state}", profile.profile, root.path);
                }
                if let Some(retention) = &profile.retention {
                    let source = if retention.configured {
                        "cleanupPeriodDays"
                    } else {
                        "the default retention"
                    };
                    let left = retention
                        .days_until_oldest_expires
                        .map(|days| format!(", oldest transcript expires in {days} days"))
                        .unwrap_or_default();
                    println!(
                        "  {}: transcripts kept {} days ({source}){left}",
                        profile.profile, retention.cleanup_period_days
                    );
                }
            }
            if let Some(run) = &agent.last_run {
                println!("  last scan {}", timestamp(run.finished));
            }
        }
        for issue in &ingest.open_issues {
            println!(
                "issue: {} {} {} ({}x)",
                issue.agent, issue.kind, issue.subject, issue.occurrences
            );
        }
        for path in ingest.protected_cwds.iter().chain(&ingest.protected_sinks) {
            println!("protected folder in use: {path}");
        }
    }
    if !diagnosis.protected_folders.is_empty() {
        println!("macOS asks for access before the daemon reads or writes in:");
        diagnosis
            .protected_folders
            .iter()
            .for_each(|path| println!("  {path}"));
    }
}

/// What `tokenmtr config` does.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Validate the config file.
    Check,
    /// Print the config file location.
    Path,
    /// Add a `[[subscriptions]]` entry, keeping the file's comments.
    AddSubscription(SubscriptionArgs),
    /// Add a `[[projects]]` rule, keeping the file's comments.
    AddRule(RuleArgs),
}

/// A subscription plan.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PlanArg {
    /// Claude Pro.
    Pro,
    /// Claude Max 5x.
    Max5x,
    /// Claude Max 20x.
    Max20x,
    /// Team.
    Team,
    /// Enterprise.
    Enterprise,
    /// Any other plan; pass `--fee`.
    Custom,
}

/// A billing period.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PeriodArg {
    /// Billed monthly.
    Monthly,
    /// Billed yearly.
    Yearly,
}

/// A currency of a fee.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FeeCurrency {
    /// US dollars.
    Usd,
    /// Euros.
    Eur,
}

/// Arguments of `tokenmtr config add-subscription`.
#[derive(Debug, Args)]
pub struct SubscriptionArgs {
    /// Unique id, for example `claude-max`.
    #[arg(long)]
    id: SubscriptionId,
    /// The plan.
    #[arg(long, value_enum)]
    plan: PlanArg,
    /// First day covered, `YYYY-MM-DD`.
    #[arg(long)]
    start: Date,
    /// First day no longer covered, `YYYY-MM-DD`.
    #[arg(long)]
    end: Option<Date>,
    /// Provider whose traffic it covers.
    #[arg(long, default_value = "anthropic")]
    provider: ProviderId,
    /// Agent whose traffic it covers; repeat for several.
    #[arg(long, default_value = "claude")]
    agent: Vec<AgentId>,
    /// Restrict it to this profile; repeat for several.
    #[arg(long)]
    profile: Vec<ProfileId>,
    /// The fee per period, for example `100`; the plan's list price when absent.
    #[arg(long)]
    fee: Option<Decimal>,
    /// Currency of `--fee`.
    #[arg(long, value_enum, default_value = "usd")]
    currency: FeeCurrency,
    /// Billing period.
    #[arg(long, value_enum, default_value = "monthly")]
    period: PeriodArg,
}

/// Arguments of `tokenmtr config add-rule`.
#[derive(Debug, Args)]
pub struct RuleArgs {
    /// Glob matched against the working directory, for example `~/work/acme/**`.
    #[arg(long = "match")]
    pattern: String,
    /// Project name to assign.
    #[arg(long)]
    project: ProjectKey,
    /// Client to assign.
    #[arg(long)]
    client: Option<String>,
}

impl SubscriptionArgs {
    fn spec(&self) -> anyhow::Result<SubscriptionSpec> {
        let plan = match self.plan {
            PlanArg::Pro => Plan::Pro,
            PlanArg::Max5x => Plan::Max5x,
            PlanArg::Max20x => Plan::Max20x,
            PlanArg::Team => Plan::Team,
            PlanArg::Enterprise => Plan::Enterprise,
            PlanArg::Custom => Plan::Custom,
        };
        let period = match self.period {
            PeriodArg::Monthly => BillingPeriod::Monthly,
            PeriodArg::Yearly => BillingPeriod::Yearly,
        };
        let fee = match self.fee {
            Some(units) => match self.currency {
                FeeCurrency::Usd => Money::Usd(UsdMicros::from_decimal(units)?),
                FeeCurrency::Eur => Money::Eur(EurMicros::from_decimal(units)?),
            },
            None => PlanCatalog::embedded()
                .context("cannot read the plan catalog")?
                .default_fee(plan, period)
                .context("the plan has no list price for that period: pass --fee")?,
        };
        Ok(SubscriptionSpec {
            id: self.id.clone(),
            provider: self.provider.clone(),
            plan,
            agents: self.agent.clone(),
            profiles: (!self.profile.is_empty()).then(|| self.profile.clone()),
            fee,
            period,
            start: self.start,
            end: self.end,
        })
    }
}

/// `tokenmtr config check|path|add-subscription|add-rule`.
pub fn config(setup: &Setup, action: &ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::Path => println!("{}", setup.config_path()),
        ConfigAction::Check => {
            tokenmtr_app::load_config(setup.config_path(), &ProcessEnv)
                .with_context(|| format!("{} is invalid", setup.config_path()))?;
            if setup.config_path().exists() {
                println!("{} is valid", setup.config_path());
            } else {
                println!("{} does not exist: the defaults apply", setup.config_path());
            }
        }
        ConfigAction::AddSubscription(args) => {
            let spec = args.spec()?;
            tokenmtr_app::add_subscription(setup.config_path(), &spec, &ProcessEnv)
                .context("cannot add the subscription")?;
            println!("added subscription {} to {}", spec.id, setup.config_path());
        }
        ConfigAction::AddRule(args) => {
            let client = args
                .client
                .clone()
                .map(ClientName::try_from)
                .transpose()
                .or_else(|error| bail!("invalid client: {error}"))?;
            let rule = RawProject {
                pattern: args.pattern.clone(),
                name: args.project.clone(),
                client,
            };
            tokenmtr_app::add_project_rule(setup.config_path(), &rule, &ProcessEnv)
                .context("cannot add the rule")?;
            println!("added rule {} to {}", args.pattern, setup.config_path());
        }
    }
    Ok(())
}
