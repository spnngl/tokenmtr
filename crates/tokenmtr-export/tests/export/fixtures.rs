//! Shared builders: domain rows with fixed ids so goldens are stable.

use std::str::FromStr;

use camino::Utf8PathBuf;
use jiff::Timestamp;
use jiff::civil::date;
use tempfile::TempDir;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    AgentId, BillingPeriod, Currency, DedupKey, Estimate, EurPerUsd, EventCost, EventId, EventKind,
    FxRate, MachineId, Money, Plan, PricingVersion, ProfileId, ProjectKey, ProviderId, QuotaWindow,
    SessionId, Subscription, SubscriptionId, SubscriptionSpec, TokenCounts, ToolCall, UsageEvent,
    UsdMicros, Utilization, WindowId,
};
use tokenmtr_store::{
    FxRateRow, FxSource, ProfiledToolCall, QuotaOutcome, QuotaPoll, QuotaReading, QuotaSample,
    Store,
};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub fn machine() -> TestResult<MachineId> {
    Ok(MachineId::from_str("00000000-0000-0000-0000-000000000001")?)
}

pub fn path_in(dir: &TempDir, name: &str) -> TestResult<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(dir.path().join(name))
        .map_err(|p| format!("non-UTF-8 {}", p.display()).into())
}

/// A fixed clock so `updated_at` is deterministic.
pub fn clock() -> Timestamp {
    Timestamp::constant(1_777_680_000, 0)
}

pub fn open_store(dir: &TempDir) -> TestResult<Store> {
    Ok(Store::open(&path_in(dir, "usage.db")?)?.with_clock(clock))
}

/// A usage event of `machine` for `session` at `at` (RFC 3339), with optional cwd and project.
pub fn usage(
    machine: &MachineId,
    session: &str,
    at: &str,
    cwd: Option<&str>,
) -> TestResult<UsageEvent> {
    let dedup_key = DedupKey::try_from(format!("{session}|{at}"))?;
    Ok(UsageEvent {
        id: EventId::new(machine, AgentId::CLAUDE, &dedup_key),
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key,
        at: at.parse()?,
        session: SessionId::try_from(session)?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: cwd.map(Utf8PathBuf::from),
        git_branch: cwd.map(|_| "feature/export".to_owned()),
        project: cwd.map(|_| ProjectKey::try_from("acme-api")).transpose()?,
        client: cwd.map(|_| "Acme".to_owned()),
        source_file: None,
        approximate: false,
        provider: Some(ProviderId::try_from("anthropic")?),
        model: tokenmtr_core::ModelId::try_from("claude-sonnet-4")?,
        kind: EventKind::Message,
        tokens: TokenCounts {
            input_uncached: 100,
            cache_read: 2_000,
            cache_write_5m: 30,
            cache_write_1h: 4,
            output: 500,
            reasoning: 0,
        },
        cost: EventCost::new(
            Some(UsdMicros::new(1_500_000)),
            Some(Estimate {
                usd: UsdMicros::new(1_234_567),
                pricing_version: PricingVersion::of_body(b"snapshot"),
            }),
        ),
        service_tier: Some("standard".to_owned()),
    })
}

/// The same event with different tokens, so the store gives it a new revision.
pub fn changed(mut event: UsageEvent) -> UsageEvent {
    event.tokens.output += 1;
    event
}

pub fn tool_call(session: &str, at: &str) -> TestResult<ProfiledToolCall> {
    Ok(ProfiledToolCall {
        profile: ProfileId::default_profile(),
        call: ToolCall {
            agent: AgentId::CLAUDE,
            tool_use_id: format!("toolu_{session}"),
            event: None,
            session: SessionId::try_from(session)?,
            at: at.parse()?,
            tool_name: "search".to_owned(),
            mcp_server: Some("docs".to_owned()),
            is_error: Some(false),
        },
    })
}

pub fn subscription() -> TestResult<Subscription> {
    Ok(Subscription::new(SubscriptionSpec {
        id: SubscriptionId::try_from("claude-max")?,
        provider: ProviderId::try_from("anthropic")?,
        plan: Plan::Max5x,
        agents: vec![AgentId::CLAUDE],
        profiles: Some(vec![ProfileId::default_profile()]),
        fee: Money::from_micros(Currency::Eur, 100_000_000),
        period: BillingPeriod::Monthly,
        start: date(2026, 1, 15),
        end: None,
    })?)
}

pub fn quota_poll(at: &str) -> TestResult<QuotaPoll> {
    Ok(QuotaPoll {
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        account: Some("me@example.com".to_owned()),
        fetched_at: at.parse()?,
        outcome: QuotaOutcome::Windows(vec![QuotaWindow {
            id: WindowId::five_hour(),
            state: QuotaWindowState {
                utilization: Utilization::new(42.5)?,
                resets_at: Some("2026-05-01T15:00:00Z".parse()?),
            },
        }]),
    })
}

pub fn quota_sample(at: &str) -> TestResult<QuotaSample> {
    let poll = quota_poll(at)?;
    let QuotaOutcome::Windows(mut windows) = poll.outcome else {
        return Err("fixture outcome is a window".into());
    };
    let window = windows.remove(0);
    Ok(QuotaSample {
        agent: poll.agent,
        profile: poll.profile,
        account: poll.account,
        fetched_at: poll.fetched_at,
        reading: QuotaReading::Window(window),
    })
}

pub fn fx_rate_row() -> TestResult<FxRateRow> {
    Ok(FxRateRow {
        rate: FxRate {
            date: date(2026, 4, 30),
            eur_per_usd: EurPerUsd::from_scaled(920_500_000)?,
        },
        source: FxSource::Ecb,
    })
}
