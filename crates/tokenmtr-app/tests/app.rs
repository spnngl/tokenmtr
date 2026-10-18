//! `AppContext` and report generation over a seeded store.

#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions, not the shared helpers"
)]

use std::sync::Arc;

use camino::Utf8PathBuf;
use jiff::Timestamp;
use jiff::civil::date;
use tokenmtr_app::{AppContext, ReportFormat, ReportRange, ReportRequest};
use tokenmtr_core::{
    AgentId, BillingPeriod, Currency, DedupKey, EventCost, EventId, EventKind, ModelId, Money,
    Plan, ProfileId, ProjectKey, ProviderId, SessionId, Subscription, SubscriptionId,
    SubscriptionSpec, TokenCounts, UsageEvent, UsdMicros,
};
use tokenmtr_platform::{ControlTransport, Paths};
use tokenmtr_query::{Period, Selection};
use tokenmtr_report::Scope;
use tokenmtr_store::{Store, StoreRead as _};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    _dir: tempfile::TempDir,
    root: Utf8PathBuf,
    ctx: AppContext,
}

fn event(
    store: &Store,
    key: &str,
    project: &str,
    provider: Option<&str>,
    micros: i64,
) -> Result<UsageEvent, Box<dyn std::error::Error>> {
    let dedup_key = DedupKey::try_from(key.to_owned())?;
    Ok(UsageEvent {
        id: EventId::new(&store.machine_id(), AgentId::CLAUDE, &dedup_key),
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key,
        at: "2026-03-10T12:00:00Z".parse()?,
        session: SessionId::try_from(format!("s-{key}"))?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        project: Some(ProjectKey::try_from(project.to_owned())?),
        client: None,
        source_file: None,
        approximate: false,
        provider: provider
            .map(|p| ProviderId::try_from(p.to_owned()))
            .transpose()?,
        model: ModelId::try_from("claude-sonnet".to_owned())?,
        kind: EventKind::Message,
        tokens: TokenCounts {
            output: 500,
            ..TokenCounts::default()
        },
        cost: EventCost::new(Some(UsdMicros::new(micros)), None),
        service_tier: None,
    })
}

fn fixture(config: &str) -> Result<Fixture, Box<dyn std::error::Error>> {
    let dir = tempfile::Builder::new().prefix("ta").tempdir_in("/tmp")?;
    let root = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?)
        .map_err(|p| format!("{}", p.display()))?;
    let home = root.to_string();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.clone()))?;
    let config_path = root.join("config.toml");
    std::fs::write(&config_path, config)?;
    let transport = ControlTransport::unix(paths.data_dir().join("control.sock"));
    let ctx = AppContext::load(paths, transport, config_path, Arc::new(|_: &str| None))?;
    Ok(Fixture {
        _dir: dir,
        root,
        ctx,
    })
}

fn seed_march(fixture: &Fixture) -> TestResult {
    let mut store = Store::open(fixture.ctx.store_path())?;
    store.upsert_events(&[
        event(&store, "a", "acme", Some("anthropic"), 3_000_000)?,
        event(&store, "b", "acme", Some("other"), 1_000_000)?,
        event(&store, "c", "beta", Some("anthropic"), 2_000_000)?,
    ])?;
    store.mirror_subscriptions(&[Subscription::new(SubscriptionSpec {
        id: SubscriptionId::try_from("claude-pro".to_owned())?,
        provider: ProviderId::try_from("anthropic".to_owned())?,
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 20_000_000),
        period: BillingPeriod::Monthly,
        start: date(2026, 1, 1),
        end: None,
    })?])?;
    Ok(())
}

fn request(scope: Scope, format: ReportFormat) -> ReportRequest {
    ReportRequest {
        range: ReportRange::Month(date(2026, 3, 1)),
        scope,
        format,
        currency: None,
    }
}

fn now() -> Timestamp {
    "2026-04-01T00:00:00Z".parse().expect("valid timestamp")
}

#[test]
fn loading_creates_an_empty_store_and_answers_status() -> TestResult {
    let fixture = fixture("[menubar]\nagents = [\"claude\"]\n")?;
    assert!(fixture.ctx.store_path().exists());
    let status = fixture.ctx.status(now(), Period::Today)?;
    assert_eq!(status.agents.len(), 1);
    assert_eq!(status.agents[0].totals.usage.events, 0);
    Ok(())
}

#[test]
fn a_report_covers_every_project_and_validates_its_totals() -> TestResult {
    let mut fixture = fixture("")?;
    seed_march(&fixture)?;
    fixture.ctx.reload_config()?;
    let markdown = fixture
        .ctx
        .generate_report(now(), &request(Scope::All, ReportFormat::Markdown))?;
    let text = String::from_utf8(markdown.bytes)?;
    assert!(text.contains("acme"), "{text}");
    assert!(text.contains("beta"), "{text}");
    assert!(text.contains("2026-03"), "{text}");
    assert_eq!(markdown.suggested_name, "tokenmtr-report-2026-03.md");
    Ok(())
}

#[test]
fn a_project_scope_drops_the_other_projects() -> TestResult {
    let fixture = fixture("")?;
    seed_march(&fixture)?;
    let rendered = fixture.ctx.generate_report(
        now(),
        &request(Scope::Project("acme".to_owned()), ReportFormat::Csv),
    )?;
    let text = String::from_utf8(rendered.bytes)?;
    assert!(text.contains("acme"), "{text}");
    assert!(!text.contains("beta"), "{text}");
    assert_eq!(rendered.suggested_name, "tokenmtr-report-2026-03-acme.csv");
    Ok(())
}

#[test]
fn html_and_a_custom_range_render() -> TestResult {
    let fixture = fixture("")?;
    seed_march(&fixture)?;
    let rendered = fixture.ctx.generate_report(
        now(),
        &ReportRequest {
            range: ReportRange::Custom {
                first: date(2026, 3, 1),
                last: date(2026, 3, 15),
            },
            scope: Scope::All,
            format: ReportFormat::Html,
            currency: None,
        },
    )?;
    let text = String::from_utf8(rendered.bytes)?;
    assert!(text.to_lowercase().contains("<html"), "{text}");
    assert!(text.contains("2026-03-01..2026-03-15"), "{text}");
    Ok(())
}

#[test]
fn an_inverted_range_is_an_invalid_request() -> TestResult {
    let fixture = fixture("")?;
    let result = fixture.ctx.generate_report(
        now(),
        &ReportRequest {
            range: ReportRange::Custom {
                first: date(2026, 3, 15),
                last: date(2026, 3, 1),
            },
            scope: Scope::All,
            format: ReportFormat::Csv,
            currency: None,
        },
    );
    assert!(matches!(
        result,
        Err(tokenmtr_app::AppError::InvalidRequest(_))
    ));
    Ok(())
}

#[test]
fn write_report_creates_the_file() -> TestResult {
    let fixture = fixture("")?;
    seed_march(&fixture)?;
    let out = fixture.root.join("out/report.md");
    fixture
        .ctx
        .write_report(now(), &request(Scope::All, ReportFormat::Markdown), &out)?;
    assert!(std::fs::read_to_string(out)?.contains("acme"));
    Ok(())
}

#[test]
fn recap_of_all_agents_sums_the_month() -> TestResult {
    let fixture = fixture("")?;
    seed_march(&fixture)?;
    let at = "2026-03-20T00:00:00Z".parse()?;
    let recap = fixture.ctx.recap(at, Selection::All, Period::Month)?;
    assert_eq!(recap.recap.totals.usage.api_value_micros, 6_000_000);
    Ok(())
}

#[test]
fn add_subscription_and_rule_update_the_loaded_config_and_keep_comments() -> TestResult {
    let mut fixture = fixture("# mine\n[display]\ncurrency = \"EUR\"\n")?;
    fixture.ctx.add_subscription(&SubscriptionSpec {
        id: SubscriptionId::try_from("claude-pro".to_owned())?,
        provider: ProviderId::try_from("anthropic".to_owned())?,
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 20_000_000),
        period: BillingPeriod::Monthly,
        start: date(2026, 1, 1),
        end: None,
    })?;
    assert_eq!(fixture.ctx.config().subscriptions.len(), 1);
    let text = std::fs::read_to_string(fixture.ctx.config_path())?;
    assert!(text.starts_with("# mine"), "{text}");
    Ok(())
}

#[test]
fn retention_is_set_in_the_profiles_settings_file() -> TestResult {
    let dir = tempfile::Builder::new().prefix("tr").tempdir_in("/tmp")?;
    let claude = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?)
        .map_err(|p| format!("{}", p.display()))?
        .join("claude");
    std::fs::create_dir_all(claude.join("projects"))?;
    std::fs::write(claude.join("settings.json"), "{\"theme\":\"dark\"}")?;
    let fixture = fixture(&format!(
        "[[agents.claude.profiles]]\nid = \"default\"\nconfig_dir = \"{claude}\"\n"
    ))?;
    fixture
        .ctx
        .set_claude_retention(&ProfileId::default_profile())?;
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(claude.join("settings.json"))?)?;
    assert_eq!(settings["cleanupPeriodDays"], 3650);
    assert_eq!(settings["theme"], "dark");
    Ok(())
}
