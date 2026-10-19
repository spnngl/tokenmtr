//! Per-model splits of session data: the models of one session, and tool calls and signals by
//! model.
//!
//! Tool calls and signals carry no model. Each one counts under the model of the latest real
//! traffic event of its session at or before it, else of the session's first event after it,
//! else under no model (`null`).

use std::collections::BTreeMap;

use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::Serialize;
use tokenmtr_core::fx::utc_date;
use tokenmtr_core::{AgentId, SessionId, TokenCounts};
use tokenmtr_store::StoreReader;

use crate::context::QueryContext;
use crate::engine::{micros_ts, to_count, usd_to};
use crate::error::QueryError;

/// The usage of one model in one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionModel {
    /// Model.
    pub model: String,
    /// Usage events.
    pub events: u64,
    /// Token mix.
    pub tokens: TokenCounts,
    /// API value in the result currency.
    pub api_value_micros: i64,
    /// True when a non-fresh exchange rate contributed.
    pub approximate: bool,
}

/// Tool calls of one tool by one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolModelCalls {
    /// Tool name.
    pub tool_name: String,
    /// MCP server, absent for built-in tools.
    pub mcp_server: Option<String>,
    /// Model the calls are attributed to; absent when the session has no usage event.
    pub model: Option<String>,
    /// Calls.
    pub calls: u64,
    /// Failed calls.
    pub errors: u64,
}

/// API errors and compactions attributed to one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalModel {
    /// Model; absent when the session has no usage event.
    pub model: Option<String>,
    /// API errors.
    pub api_errors: u64,
    /// Compactions.
    pub compactions: u64,
}

const SESSION_MODELS: &str = "SELECT model, \
     at_us - ((at_us % 86400000000) + 86400000000) % 86400000000, COUNT(*), \
     SUM(input_uncached), SUM(cache_read), SUM(cache_write_5m), SUM(cache_write_1h), \
     SUM(output), SUM(reasoning), COALESCE(SUM(cost_api_usd_micros), 0) FROM usage_event \
     WHERE agent = ? AND session = ? AND superseded = 0 \
     AND kind NOT IN ('daily_aggregate', 'extra_usage') GROUP BY 1, 2";

/// Every model of one session, highest API value first. Real traffic only, like the timeline.
pub(crate) fn session_models(
    reader: &StoreReader,
    ctx: &QueryContext,
    agent: AgentId,
    session: &SessionId,
) -> Result<Vec<SessionModel>, QueryError> {
    let mut statement = reader.connection().prepare_cached(SESSION_MODELS)?;
    let mut rows = statement.query([agent.as_str(), session.as_str()])?;
    let mut by_model: BTreeMap<String, SessionModel> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let model: String = row.get(0)?;
        let day: i64 = row.get(1)?;
        let (value, approximate) = usd_to(
            ctx.currency(),
            ctx.fx(),
            utc_date(micros_ts(day)?),
            row.get(9)?,
        )?;
        let count = |i: usize| -> Result<u64, QueryError> { to_count(row.get(i)?) };
        let entry = by_model
            .entry(model.clone())
            .or_insert_with(|| SessionModel {
                model,
                events: 0,
                tokens: TokenCounts::default(),
                api_value_micros: 0,
                approximate: false,
            });
        entry.events += count(2)?;
        let t = &mut entry.tokens;
        t.input_uncached += count(3)?;
        t.cache_read += count(4)?;
        t.cache_write_5m += count(5)?;
        t.cache_write_1h += count(6)?;
        t.output += count(7)?;
        t.reasoning += count(8)?;
        entry.api_value_micros += value;
        entry.approximate |= approximate;
    }
    let mut models: Vec<SessionModel> = by_model.into_values().collect();
    models.sort_by(|a, b| {
        b.api_value_micros
            .cmp(&a.api_value_micros)
            .then_with(|| a.model.cmp(&b.model))
    });
    Ok(models)
}

/// The model of the row `x` (a `tool_call` or `session_signal`), as a SQL expression.
const MODEL_OF: &str = "COALESCE(\
     (SELECT u.model FROM usage_event u WHERE u.session = x.session AND u.agent = x.agent \
     AND u.at_us <= x.at_us AND u.superseded = 0 \
     AND u.kind NOT IN ('daily_aggregate', 'extra_usage') ORDER BY u.at_us DESC LIMIT 1), \
     (SELECT u.model FROM usage_event u WHERE u.session = x.session AND u.agent = x.agent \
     AND u.at_us > x.at_us AND u.superseded = 0 \
     AND u.kind NOT IN ('daily_aggregate', 'extra_usage') ORDER BY u.at_us LIMIT 1))";

/// Tool calls per tool and model, most called first. `conditions` select on `tool_call`.
pub(crate) fn tool_models(
    reader: &StoreReader,
    conditions: &str,
    params: Vec<Value>,
) -> Result<Vec<ToolModelCalls>, QueryError> {
    let mut statement = reader.connection().prepare_cached(&format!(
        "SELECT tool_name, mcp_server, {MODEL_OF}, COUNT(*), COALESCE(SUM(is_error = 1), 0) \
         FROM tool_call x WHERE {conditions} GROUP BY 1, 2, 3 ORDER BY 4 DESC, 1, 2, 3"
    ))?;
    let mut rows = statement.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(ToolModelCalls {
            tool_name: row.get(0)?,
            mcp_server: row.get(1)?,
            model: row.get(2)?,
            calls: to_count(row.get(3)?)?,
            errors: to_count(row.get(4)?)?,
        });
    }
    Ok(out)
}

/// API errors and compactions per model, most signals first. `conditions` select on
/// `session_signal`.
pub(crate) fn signal_models(
    reader: &StoreReader,
    conditions: &str,
    params: Vec<Value>,
) -> Result<Vec<SignalModel>, QueryError> {
    let mut statement = reader.connection().prepare_cached(&format!(
        "SELECT {MODEL_OF} AS m, COALESCE(SUM(kind = 'api_error'), 0) AS e, \
         COALESCE(SUM(kind = 'compaction'), 0) AS c FROM session_signal x WHERE {conditions} \
         GROUP BY m ORDER BY e + c DESC, m"
    ))?;
    let mut rows = statement.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SignalModel {
            model: row.get(0)?,
            api_errors: to_count(row.get(1)?)?,
            compactions: to_count(row.get(2)?)?,
        });
    }
    Ok(out)
}
