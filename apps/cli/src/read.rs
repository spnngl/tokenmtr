//! Read-only commands: `status`, `recap`, `alerts`, `quota`, `fx`, `pricing`. They open the store
//! directly and work with the daemon down.

use anyhow::Context;
use clap::{Args, Subcommand, ValueEnum};
use jiff::SignedDuration;
use tokenmtr_app::{AppContext, FxSummary, PricingSummary, QuotaReport};
use tokenmtr_core::{AgentId, QuotaEta};
use tokenmtr_query::{
    AgentRecap, AlertView, DaemonState, HealthLevel, Period, QuotaMeter, Selection, SinkState,
    SystemHealth,
};

use crate::format::{money, percent, print_json, seconds, timestamp};
use crate::setup::{Setup, now};

/// The period of a recap.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PeriodArg {
    /// Since local midnight.
    Today,
    /// Since the start of the week (Monday).
    Week,
    /// Since the first of the month.
    Month,
}

impl From<PeriodArg> for Period {
    fn from(period: PeriodArg) -> Self {
        match period {
            PeriodArg::Today => Self::Today,
            PeriodArg::Week => Self::Week,
            PeriodArg::Month => Self::Month,
        }
    }
}

/// Arguments of `tokenmtr status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Print the `status_schema = 1` JSON document (docs/cli-json.md).
    #[arg(long)]
    json: bool,
    /// Period of the money and token totals.
    #[arg(long, value_enum, default_value = "today")]
    period: PeriodArg,
}

/// Arguments of `tokenmtr recap`.
#[derive(Debug, Args)]
pub struct RecapArgs {
    /// Print the `status_schema = 1` JSON document (docs/cli-json.md).
    #[arg(long)]
    json: bool,
    /// Agent to recap, for example `claude` or `pi`; all agents summed when absent.
    #[arg(long)]
    agent: Option<AgentId>,
    /// Period of the totals.
    #[arg(long, value_enum, default_value = "today")]
    period: PeriodArg,
}

/// Arguments of `tokenmtr alerts`.
#[derive(Debug, Args)]
pub struct AlertsArgs {
    /// Print JSON.
    #[arg(long)]
    json: bool,
    /// How many days back to look.
    #[arg(long, default_value_t = 30)]
    days: u32,
    /// Most alerts to print.
    #[arg(long, default_value_t = 50)]
    limit: u32,
}

/// Arguments of `tokenmtr quota`, `fx` and `pricing`.
#[derive(Debug, Args)]
pub struct JsonArgs {
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

/// `tokenmtr status`: system health and one recap per followed agent.
pub fn status(setup: &Setup, args: &StatusArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let status = ctx.status(now(), args.period.into())?;
    if args.json {
        return print_json(&status);
    }
    render_system(&status.system);
    if status.agents.is_empty() {
        println!("no followed agents (set menubar.agents in the config)");
    }
    status.agents.iter().for_each(render_recap);
    Ok(())
}

/// `tokenmtr recap`: one tab of the menu-bar popover.
pub fn recap(setup: &Setup, args: &RecapArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let selection = args.agent.map_or(Selection::All, Selection::Agent);
    let recap = ctx.recap(now(), selection, args.period.into())?;
    if args.json {
        return print_json(&recap);
    }
    render_system(&recap.system);
    render_recap(&recap.recap);
    Ok(())
}

/// `tokenmtr alerts`: the alerts that fired.
pub fn alerts(setup: &Setup, args: &AlertsArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let since = now()
        .checked_sub(SignedDuration::from_hours(24 * i64::from(args.days)))
        .context("the look-back window is out of range")?;
    let alerts = ctx.alerts(since, args.limit)?;
    if args.json {
        return print_json(&alerts);
    }
    if alerts.is_empty() {
        println!("no alerts in the last {} days", args.days);
    }
    alerts.iter().for_each(render_alert);
    Ok(())
}

/// Arguments of `tokenmtr quota`.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct QuotaArgs {
    #[command(subcommand)]
    action: Option<QuotaAction>,
    #[command(flatten)]
    show: JsonArgs,
}

/// What `tokenmtr quota` does besides showing the windows.
#[derive(Debug, Subcommand)]
enum QuotaAction {
    /// Let the daemon read the Claude credentials in the Keychain: macOS asks once, choose
    /// "Always Allow".
    Connect(JsonArgs),
}

/// `tokenmtr quota`: the latest quota windows and the detected plans, or `connect`.
pub fn quota(setup: &Setup, args: &QuotaArgs) -> anyhow::Result<()> {
    match &args.action {
        None => show_quota(setup, &args.show),
        Some(QuotaAction::Connect(json)) => crate::data::quota_connect(setup, json.json),
    }
}

fn show_quota(setup: &Setup, args: &JsonArgs) -> anyhow::Result<()> {
    let report = setup.context()?.quota(now())?;
    if args.json {
        return print_json(&report);
    }
    render_quota(&report);
    Ok(())
}

/// `tokenmtr fx`: the latest FX rate.
pub fn fx(setup: &Setup, args: &JsonArgs) -> anyhow::Result<()> {
    let summary = setup.context()?.fx(now())?;
    if args.json {
        return print_json(&summary);
    }
    render_fx(&summary);
    Ok(())
}

/// `tokenmtr pricing`: the active snapshot and the models it cannot price.
pub fn pricing(setup: &Setup, args: &JsonArgs) -> anyhow::Result<()> {
    let ctx: AppContext = setup.context()?;
    let summary = ctx.pricing(now())?;
    if args.json {
        return print_json(&summary);
    }
    render_pricing(&summary);
    Ok(())
}

fn render_system(system: &SystemHealth) {
    let level = match system.level {
        HealthLevel::Ok => "ok",
        HealthLevel::Warning => "warning",
        HealthLevel::Error => "error",
    };
    let daemon = &system.daemon;
    let state = match (daemon.state, daemon.age_secs) {
        (DaemonState::Running, Some(age)) => format!("running (heartbeat {} ago)", seconds(age)),
        (DaemonState::Running, None) => "running".to_owned(),
        (DaemonState::Stale, Some(age)) => {
            format!("not running (last heartbeat {} ago)", seconds(age))
        }
        (DaemonState::Stale, None) => "not running".to_owned(),
        (DaemonState::NeverStarted, _) => "never started".to_owned(),
    };
    println!("health: {level}");
    println!("daemon: {state}");
    for sink in &system.sinks {
        let state = match &sink.state {
            SinkState::Healthy => "healthy".to_owned(),
            SinkState::Lagging => format!("lagging ({} rows waiting)", sink.lag_rows),
            SinkState::Failing { since, last_error } => {
                format!("failing since {}: {last_error}", timestamp(*since))
            }
            SinkState::Disabled => "disabled".to_owned(),
        };
        println!("sink {}: {state}", sink.sink);
    }
}

fn render_recap(recap: &AgentRecap) {
    let name = recap
        .agent
        .map_or_else(|| "all agents".to_owned(), |agent| agent.to_string());
    let totals = &recap.totals;
    let currency = recap.currency;
    println!();
    println!(
        "{name}, {} ({} to {})",
        period_name(recap.period),
        recap.range.start(),
        recap.range.end()
    );
    println!(
        "  paid {}, API value {}, {} tokens, {} sessions",
        money(totals.usage.paid_micros, currency),
        money(totals.usage.api_value_micros, currency),
        totals.usage.total_tokens,
        totals.sessions
    );
    if recap.currency_fallback {
        println!("  (no FX rate yet: amounts are in USD)");
    }
    if let Some(roi) = totals.roi {
        println!("  subscription ROI {roi:.2}x");
    }
    println!(
        "  projected paid {}",
        money(recap.projected_paid_micros, currency)
    );
    if let Some(model) = &recap.top_model {
        println!("  top model {model}");
    }
    if let Some(live) = &recap.live {
        println!(
            "  live: {} tokens/min, {} active sessions",
            live.tokens_per_minute, live.active_sessions
        );
    }
    if let Some(at) = recap.last_activity {
        println!("  last activity {}", timestamp(at));
    }
    recap
        .quota
        .iter()
        .for_each(|meter| render_meter(meter, "  "));
    for budget in &recap.budgets {
        println!(
            "  budget {}: {}% of {} spent ({}), {}% projected",
            budget.id,
            budget.percent,
            money(budget.limit_micros, budget.currency),
            money(budget.spent_micros, budget.currency),
            budget.projected_percent
        );
    }
}

const fn period_name(period: Period) -> &'static str {
    match period {
        Period::Today => "today",
        Period::Week => "this week",
        Period::Month => "this month",
    }
}

fn render_meter(meter: &QuotaMeter, indent: &str) {
    let account = meter
        .account
        .as_deref()
        .map(|account| format!(" ({account})"))
        .unwrap_or_default();
    println!(
        "{indent}quota {}/{}{account}: {}, polled {} ago",
        meter.agent,
        meter.profile,
        meter.status,
        seconds(meter.age_secs)
    );
    for window in &meter.windows {
        let resets = window
            .resets_in_secs
            .map(|secs| format!(", resets in {}", seconds(secs)))
            .unwrap_or_default();
        let eta = match window.eta {
            QuotaEta::WillHit { at } => format!(", will run out at {}", timestamp(at)),
            QuotaEta::WontHit => ", on track to last".to_owned(),
            QuotaEta::NotEnoughData => String::new(),
        };
        let spend = window
            .spend
            .map(|s| {
                format!(
                    " ({} of {} this month)",
                    money(s.spent_micros, s.currency),
                    money(s.limit_micros, s.currency)
                )
            })
            .unwrap_or_default();
        println!(
            "{indent}  {}: {} used{spend}{resets}{eta}",
            window.window,
            percent(window.used_percent)
        );
    }
}

fn render_alert(alert: &AlertView) {
    println!("{}  {}", timestamp(alert.fired_at), alert.dedup_key);
}

fn render_quota(report: &QuotaReport) {
    if report.meters.is_empty() {
        println!("no quota polls yet (the daemon polls providers while it runs)");
    }
    report
        .meters
        .iter()
        .for_each(|meter| render_meter(meter, ""));
    for account in &report.accounts {
        let plan = account
            .detected_plan
            .map_or_else(|| "unknown".to_owned(), |plan| format!("{plan:?}"));
        let tier = account
            .rate_limit_tier
            .as_deref()
            .map(|tier| format!(", tier {tier}"))
            .unwrap_or_default();
        println!(
            "plan {}/{}: {plan}{tier} (fetched {})",
            account.agent,
            account.profile,
            timestamp(account.fetched_at)
        );
    }
}

/// `0.9000` from a rate scaled by 10⁹.
fn rate(scaled: i64) -> String {
    let ten_thousandths = (i128::from(scaled) + 50_000) / 100_000;
    format!(
        "{}.{:04}",
        ten_thousandths / 10_000,
        ten_thousandths % 10_000
    )
}

fn render_fx(summary: &FxSummary) {
    if let Some(fixed) = summary.fixed_eur_per_usd_scaled {
        println!(
            "fixed rate (fx.fixed_eur_per_usd): 1 USD = {} EUR",
            rate(fixed)
        );
    }
    match &summary.latest {
        Some(latest) => println!(
            "latest ECB rate: 1 USD = {} EUR on {} ({})",
            rate(latest.eur_per_usd_scaled),
            latest.date,
            latest.source
        ),
        None => println!("no FX rate stored yet (the daemon fetches ECB rates while it runs)"),
    }
}

fn render_pricing(summary: &PricingSummary) {
    match &summary.snapshot {
        Some(snapshot) => println!(
            "pricing snapshot {} ({} bytes), fetched {}{}",
            snapshot.version,
            snapshot.bytes,
            timestamp(snapshot.fetched_at),
            snapshot
                .source
                .as_deref()
                .map(|source| format!(" from {source}"))
                .unwrap_or_default()
        ),
        None => println!("no pricing snapshot stored: the embedded catalog applies"),
    }
    if summary.unpriced_models.is_empty() {
        println!("every model is priced");
    }
    for model in &summary.unpriced_models {
        println!(
            "unpriced: {} ({} events)",
            model.model, model.unpriced_events
        );
    }
}
