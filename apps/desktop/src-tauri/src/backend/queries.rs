//! One function per filtered dashboard query: parse the request, run the query crate, serialise.

use jiff::{Span, Timestamp};
use serde::Serialize;
use serde_json::Value;
use tokenmtr_app::AppContext;
use tokenmtr_core::{AgentId, Currency, ProfileId, SessionId};
use tokenmtr_query::{
    AgentRecap, Bucket, DateRange, Filters, GroupBy, Period, QueryContext, QueryError, Selection,
    Status,
};
use tokenmtr_store::StoreReader;

use crate::error::CommandError;
use crate::request::QueryRequest;

/// Points a chart may hold before it switches to weekly or monthly buckets.
const MAX_SERIES_POINTS: u32 = 90;
/// Rows the unresolved-working-directory table may hold.
const UNRESOLVED_LIMIT: u32 = 200;
/// Alerts the Settings page lists, and how far back.
const ALERT_LIMIT: u32 = 100;
const ALERT_DAYS: i64 = 30;
/// Backup attempts listed.
const BACKUP_LIMIT: u32 = 20;
/// Parse issues listed.
const ISSUE_LIMIT: u32 = 200;
/// Days of FX history listed.
const FX_DAYS: i64 = 365;

fn json<T: Serialize>(result: Result<T, QueryError>) -> Result<Value, CommandError> {
    serde_json::to_value(result?).map_err(CommandError::internal)
}

/// Parses `request` against the store and runs `query` with the typed filters.
fn filtered<T: Serialize>(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
    query: impl FnOnce(&StoreReader, &QueryContext, &Filters) -> Result<T, QueryError>,
) -> Result<Value, CommandError> {
    let ctx = tokenmtr_app::query_context(app.config(), app.reader(), now, Some(request.currency))?;
    let filters = Filters::parse(&request.filters, &ctx)?;
    json(query(app.reader(), &ctx, &filters))
}

fn default_context(app: &AppContext, now: Timestamp) -> Result<QueryContext, CommandError> {
    Ok(tokenmtr_app::query_context(
        app.config(),
        app.reader(),
        now,
        None,
    )?)
}

/// `overview`.
pub fn overview(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::overview)
}

/// `series`.
pub fn series(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
    group_by: GroupBy,
) -> Result<Value, CommandError> {
    filtered(app, now, request, |reader, ctx, filters| {
        let bucket = Bucket::pick(&filters.range(), MAX_SERIES_POINTS);
        tokenmtr_query::series(reader, ctx, filters, bucket, group_by)
    })
}

/// `models`.
pub fn models(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::models)
}

/// `projects`.
pub fn projects(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::projects)
}

/// `branches`.
pub fn branches(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::branches)
}

/// `unresolved_cwds`.
pub fn unresolved_cwds(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, |reader, ctx, filters| {
        tokenmtr_query::unresolved_cwds(reader, ctx, filters, UNRESOLVED_LIMIT)
    })
}

/// `heatmap`.
pub fn heatmap(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::heatmap)
}

/// `tools`.
pub fn tools(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::tools)
}

/// `signals`.
pub fn signals(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::signals)
}

/// `billing`.
pub fn billing(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::billing)
}

/// `quota_latest`.
pub fn quota_latest(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    filtered(app, now, request, tokenmtr_query::quota_latest)
}

/// `sessions`.
pub fn sessions(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
    page: u32,
    page_size: u32,
) -> Result<Value, CommandError> {
    filtered(app, now, request, |reader, ctx, filters| {
        tokenmtr_query::sessions(reader, ctx, filters, page, page_size)
    })
}

/// `quota_history`: one entry per window of every selected profile of `agent`.
pub fn quota_history(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
    agent: AgentId,
) -> Result<Value, CommandError> {
    let configured: Vec<ProfileId> = app
        .config()
        .agents
        .get(&agent)
        .map(|config| config.profiles.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default();
    filtered(app, now, request, |reader, ctx, filters| {
        let profiles = if filters.restricts_profiles() {
            filters.profiles_of(agent)
        } else {
            configured.clone()
        };
        let mut out = Vec::new();
        for profile in &profiles {
            out.extend(tokenmtr_query::quota_history(
                reader,
                ctx,
                agent,
                profile.as_str(),
                filters.range(),
            )?);
        }
        Ok(out)
    })
}

/// `advisor`: one report per quota-polled profile in scope.
pub fn advisor(
    app: &AppContext,
    now: Timestamp,
    request: &QueryRequest,
) -> Result<Value, CommandError> {
    let headroom = app.config().advisor.headroom.get();
    let enabled: Vec<AgentId> = app
        .config()
        .enabled_agents()
        .filter(|agent| crate::backend::shell::capabilities(*agent).quota)
        .collect();
    let configured = |agent: AgentId| -> Vec<ProfileId> {
        app.config()
            .agents
            .get(&agent)
            .map(|config| config.profiles.iter().map(|p| p.id.clone()).collect())
            .unwrap_or_default()
    };
    filtered(app, now, request, |reader, ctx, filters| {
        let mut out = Vec::new();
        for agent in &enabled {
            if !filters.agent_list().is_empty() && !filters.agent_list().contains(agent) {
                continue;
            }
            let profiles = if filters.restricts_profiles() {
                filters.profiles_of(*agent)
            } else {
                configured(*agent)
            };
            for profile in &profiles {
                out.push(tokenmtr_query::advisor(
                    reader, ctx, *agent, profile, None, headroom,
                )?);
            }
        }
        Ok(out)
    })
}

/// `session_detail`.
pub fn session_detail(
    app: &AppContext,
    now: Timestamp,
    agent: AgentId,
    session: &SessionId,
    currency: Currency,
) -> Result<Value, CommandError> {
    let ctx = tokenmtr_app::query_context(app.config(), app.reader(), now, Some(currency))?;
    json(tokenmtr_query::session_detail(
        app.reader(),
        &ctx,
        agent,
        session,
    ))
}

/// `budgets`.
pub fn budgets(
    app: &AppContext,
    now: Timestamp,
    currency: Currency,
) -> Result<Value, CommandError> {
    let ctx = tokenmtr_app::query_context(app.config(), app.reader(), now, Some(currency))?;
    json(tokenmtr_query::budgets(
        app.reader(),
        &ctx,
        &app.budget_specs(),
    ))
}

/// `alerts`: the last thirty days, newest first.
pub fn alerts(app: &AppContext, now: Timestamp) -> Result<Value, CommandError> {
    let since = now
        .checked_sub(Span::new().hours(ALERT_DAYS * 24))
        .map_err(CommandError::internal)?;
    json(tokenmtr_query::alerts(app.reader(), since, ALERT_LIMIT))
}

/// `backups`: recorded attempts, newest first.
pub fn backups(app: &AppContext) -> Result<Value, CommandError> {
    json(tokenmtr_query::backup_runs(app.reader(), BACKUP_LIMIT))
}

/// `fx_rates`: the last year of stored rates.
pub fn fx_rates(app: &AppContext, now: Timestamp) -> Result<Value, CommandError> {
    let ctx = default_context(app, now)?;
    let today = ctx.today();
    let start = today
        .checked_sub(Span::new().days(FX_DAYS))
        .map_err(CommandError::internal)?;
    let end = today
        .checked_add(Span::new().days(1))
        .map_err(CommandError::internal)?;
    let range = DateRange::new(start, end)?;
    json(tokenmtr_query::fx_rates(app.reader(), range))
}

/// `ingest_status`: the last run of every agent.
pub fn ingest_status(app: &AppContext) -> Result<Value, CommandError> {
    json(tokenmtr_query::ingest_status(app.reader()))
}

/// `parse_issues`: open issues, newest first.
pub fn parse_issues(app: &AppContext) -> Result<Value, CommandError> {
    json(tokenmtr_query::parse_issues(
        app.reader(),
        false,
        ISSUE_LIMIT,
    ))
}

/// The status document at `period` in `currency`, with the followed agents' recaps.
///
/// # Errors
///
/// Store and query failures.
pub fn status(
    app: &AppContext,
    now: Timestamp,
    currency: Currency,
    period: Period,
) -> Result<Status, CommandError> {
    let ctx = tokenmtr_app::query_context(app.config(), app.reader(), now, Some(currency))?;
    Ok(tokenmtr_query::status(
        app.reader(),
        &ctx,
        &app.recap_config(),
        &app.config().menubar.agents,
        period,
    )?)
}

/// `system_health`: the daemon and the sinks.
pub fn system_health(app: &AppContext, now: Timestamp) -> Result<Value, CommandError> {
    let ctx = default_context(app, now)?;
    let status =
        tokenmtr_query::status(app.reader(), &ctx, &app.recap_config(), &[], Period::Today)?;
    serde_json::to_value(status.system).map_err(CommandError::internal)
}

/// `recap`: the tabs of the tray window. With several followed agents an All tab leads.
pub fn recap(
    app: &AppContext,
    now: Timestamp,
    currency: Currency,
    period: Period,
) -> Result<Value, CommandError> {
    let status = status(app, now, currency, period)?;
    let all = if status.agents.len() > 1 {
        let ctx = tokenmtr_app::query_context(app.config(), app.reader(), now, Some(currency))?;
        let all = tokenmtr_query::recap(
            app.reader(),
            &ctx,
            &app.recap_config(),
            Selection::All,
            period,
        )?;
        Some(all.recap)
    } else {
        None
    };
    Ok(recap_document(status, all))
}

/// The `recap` response: `status` with the All tab `all` (if any) before the agents' tabs.
pub(crate) fn recap_document(status: Status, all: Option<AgentRecap>) -> Value {
    let mut tabs = Vec::with_capacity(status.agents.len() + 1);
    tabs.extend(all);
    tabs.extend(status.agents);
    serde_json::json!({
        "status_schema": status.status_schema,
        "generated_at": status.generated_at,
        "system": status.system,
        "tabs": tabs,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::backend::fixture::{CONFIG, seeded};

    fn request(currency: &str) -> Result<QueryRequest, serde_json::Error> {
        serde_json::from_value(json!({
            "filters": { "range": { "kind": "last_days", "days": 7 } },
            "currency": currency,
        }))
    }

    #[test]
    fn overview_totals_the_seeded_events() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let totals = overview(&app, fixture.now, &request("USD")?)?;
        assert_eq!(totals["events"], 2);
        assert_eq!(totals["currency"], "USD");
        assert_eq!(totals["api_value_micros"], 5_000_000);
        Ok(())
    }

    #[test]
    fn models_and_sessions_list_the_seeded_session() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let models = models(&app, fixture.now, &request("USD")?)?;
        assert_eq!(models["currency"], "USD");
        assert_eq!(models["rows"][0]["model"], "claude-sonnet-4");
        let page = sessions(&app, fixture.now, &request("USD")?, 0, 10)?;
        assert_eq!(page["total"], 1);
        let session = SessionId::try_from("s1")?;
        let detail = session_detail(&app, fixture.now, AgentId::CLAUDE, &session, Currency::Usd)?;
        assert_eq!(detail["events"].as_array().map(Vec::len), Some(2));
        Ok(())
    }

    #[test]
    fn series_buckets_by_day() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let series = series(&app, fixture.now, &request("USD")?, GroupBy::None)?;
        assert!(series.is_object(), "{series}");
        Ok(())
    }

    #[test]
    fn recap_has_one_tab_per_followed_agent() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        let snapshot = recap(&app, fixture.now, Currency::Usd, Period::Today)?;
        assert_eq!(snapshot["status_schema"], 1);
        let tabs = snapshot["tabs"].as_array().map_or(0, Vec::len);
        assert_eq!(
            tabs,
            app.config().menubar.agents.len() + usize::from(app.config().menubar.agents.len() > 1)
        );
        assert_eq!(snapshot["tabs"][0]["totals"]["events"], 2);
        Ok(())
    }

    #[test]
    fn status_alerts_and_health_read_an_empty_tail() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let app = fixture.backend.context()?;
        assert_eq!(alerts(&app, fixture.now)?, json!([]));
        assert_eq!(backups(&app)?, json!([]));
        assert_eq!(system_health(&app, fixture.now)?["level"], "ok");
        assert!(budgets(&app, fixture.now, Currency::Usd)?.is_array());
        Ok(())
    }
}
