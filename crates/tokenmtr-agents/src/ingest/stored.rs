//! Reads of stored rows the public store API has no lookup for (D11, D32, D38, D53).
//!
//! The ingestor needs point lookups: "what estimate does this event already carry", "which tool
//! call does this result belong to", "when did this profile's first transcript event happen".
//! `tokenmtr-store` offers only revision-ordered pages, so these go through the read-only
//! connection of [`StoreReader`](tokenmtr_store::StoreReader). All schema knowledge of the engine
//! is in this file; its tests run against a real store, so a migration that renames a column
//! fails here first.

use camino::Utf8PathBuf;
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, named_params};
use tokenmtr_core::{
    AgentId, DedupKey, Estimate, EventId, PricingVersion, ProfileId, SessionId, TokenCounts,
    ToolCall, UsdMicros,
};
use tokenmtr_store::ProfiledToolCall;

use super::error::IngestError;

/// What an already stored event says about its price (D11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPricing {
    /// Token columns as stored.
    pub tokens: TokenCounts,
    /// Model as stored.
    pub model: String,
    /// Provider as stored.
    pub provider: Option<String>,
    /// Service tier as stored.
    pub service_tier: Option<String>,
    /// The estimate and its snapshot, `None` when the event is unpriced.
    pub estimate: Option<Estimate>,
}

fn count(row: &Row<'_>, column: &str) -> Result<u64, rusqlite::Error> {
    let value: i64 = row.get(column)?;
    Ok(u64::try_from(value).unwrap_or(0))
}

/// The stored pricing facts of the event `(agent, dedup_key)`.
pub fn event_pricing(
    conn: &Connection,
    agent: AgentId,
    key: &DedupKey,
) -> Result<Option<StoredPricing>, IngestError> {
    let mut statement = conn.prepare_cached(
        "SELECT input_uncached, cache_read, cache_write_5m, cache_write_1h, output, reasoning, \
         model, provider, service_tier, cost_estimated_usd_micros, pricing_version \
         FROM usage_event WHERE agent = :agent AND dedup_key = :key",
    )?;
    let row = statement
        .query_row(
            named_params! {":agent": agent.as_str(), ":key": key.as_str()},
            |row| {
                let estimated: Option<i64> = row.get("cost_estimated_usd_micros")?;
                let version: Option<String> = row.get("pricing_version")?;
                Ok((
                    TokenCounts {
                        input_uncached: count(row, "input_uncached")?,
                        cache_read: count(row, "cache_read")?,
                        cache_write_5m: count(row, "cache_write_5m")?,
                        cache_write_1h: count(row, "cache_write_1h")?,
                        output: count(row, "output")?,
                        reasoning: count(row, "reasoning")?,
                    },
                    row.get::<_, String>("model")?,
                    row.get::<_, Option<String>>("provider")?,
                    row.get::<_, Option<String>>("service_tier")?,
                    estimated.zip(version),
                ))
            },
        )
        .optional()?;
    let Some((tokens, model, provider, service_tier, estimate)) = row else {
        return Ok(None);
    };
    let estimate = estimate
        .map(|(usd, version)| {
            Ok::<_, IngestError>(Estimate {
                usd: UsdMicros::new(usd),
                pricing_version: PricingVersion::try_from(version)?,
            })
        })
        .transpose()?;
    Ok(Some(StoredPricing {
        tokens,
        model,
        provider,
        service_tier,
        estimate,
    }))
}

/// The stored tool call `(agent, tool_use_id)`.
pub fn tool_call(
    conn: &Connection,
    agent: AgentId,
    tool_use_id: &str,
) -> Result<Option<ProfiledToolCall>, IngestError> {
    let mut statement = conn.prepare_cached(
        "SELECT profile, event_id, session, at_us, tool_name, mcp_server, is_error \
         FROM tool_call WHERE agent = :agent AND tool_use_id = :id",
    )?;
    let row = statement
        .query_row(
            named_params! {":agent": agent.as_str(), ":id": tool_use_id},
            |row| {
                Ok((
                    row.get::<_, String>("profile")?,
                    row.get::<_, Option<[u8; 16]>>("event_id")?,
                    row.get::<_, String>("session")?,
                    row.get::<_, i64>("at_us")?,
                    row.get::<_, String>("tool_name")?,
                    row.get::<_, Option<String>>("mcp_server")?,
                    row.get::<_, Option<bool>>("is_error")?,
                ))
            },
        )
        .optional()?;
    let Some((profile, event, session, at_us, tool_name, mcp_server, is_error)) = row else {
        return Ok(None);
    };
    Ok(Some(ProfiledToolCall {
        profile: ProfileId::try_from(profile)?,
        call: ToolCall {
            agent,
            tool_use_id: tool_use_id.to_owned(),
            event: event.map(EventId::from_bytes),
            session: SessionId::try_from(session)?,
            at: Timestamp::from_microsecond(at_us)
                .map_err(|err| IngestError::Corrupt(format!("tool call time: {err}")))?,
            tool_name,
            mcp_server,
            is_error,
        },
    }))
}

/// When the first non-aggregate event of `(agent, profile)` happened (D38).
pub fn earliest_transcript_event(
    conn: &Connection,
    agent: AgentId,
    profile: &ProfileId,
) -> Result<Option<Timestamp>, IngestError> {
    let at: Option<i64> = conn
        .prepare_cached(
            "SELECT MIN(at_us) FROM usage_event \
             WHERE agent = :agent AND profile = :profile AND kind != 'daily_aggregate'",
        )?
        .query_row(
            named_params! {":agent": agent.as_str(), ":profile": profile.as_str()},
            |row| row.get(0),
        )?;
    at.map(|us| {
        Timestamp::from_microsecond(us)
            .map_err(|err| IngestError::Corrupt(format!("event time: {err}")))
    })
    .transpose()
}

/// One open parse issue, as the doctor lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OpenIssue {
    /// Agent the issue belongs to.
    pub agent: String,
    /// Issue kind as stored (`format_drift`, `protected_folder`, ...).
    pub kind: String,
    /// File or directory the issue is about.
    pub subject: String,
    /// How often it was seen.
    pub occurrences: u64,
    /// Key skeleton or reason text; never line content.
    pub sample: Option<String>,
}

/// Every open parse issue, oldest first.
pub fn open_issues(conn: &Connection) -> Result<Vec<OpenIssue>, IngestError> {
    let mut statement = conn.prepare_cached(
        "SELECT agent, kind, subject, occurrences, sample FROM parse_issue \
         WHERE resolved_at_us IS NULL ORDER BY first_seen_us, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(OpenIssue {
            agent: row.get("agent")?,
            kind: row.get("kind")?,
            subject: row.get("subject")?,
            occurrences: count(row, "occurrences")?,
            sample: row.get("sample")?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The `skipped` JSON of the latest ingest run of `agent` that parsed something, with its time.
pub fn last_ingest_skipped(
    conn: &Connection,
    agent: AgentId,
) -> Result<Option<(Timestamp, String)>, IngestError> {
    let row: Option<(i64, String)> = conn
        .prepare_cached(
            "SELECT finished_at_us, skipped FROM ingest_run WHERE agent = :agent \
             ORDER BY id DESC LIMIT 1",
        )?
        .query_row(named_params! {":agent": agent.as_str()}, |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    row.map(|(us, json)| {
        Timestamp::from_microsecond(us)
            .map(|at| (at, json))
            .map_err(|err| IngestError::Corrupt(format!("run time: {err}")))
    })
    .transpose()
}

/// Distinct cwds of stored events that have no stored resolution yet.
pub fn unresolved_cwds(conn: &Connection) -> Result<Vec<Utf8PathBuf>, IngestError> {
    let mut statement = conn.prepare_cached(
        "SELECT DISTINCT cwd FROM usage_event \
         WHERE cwd IS NOT NULL AND cwd NOT IN (SELECT cwd FROM project_map) ORDER BY cwd",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(Utf8PathBuf::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use tokenmtr_store::{Store, StoreRead as _, StoreReader};

    use super::*;
    use crate::testkit::{parsed_event, utf8_dir};

    #[test]
    fn schema_guard_every_query_runs_against_a_real_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = utf8_dir(&dir).join("store.db");
        let mut store = Store::open(&path).unwrap();
        let machine = store.machine_id();
        let event = parsed_event("k1", "2025-10-01T10:00:00Z", 100, 50);
        let usage = event.clone().into_usage_event(&machine, None);
        store.upsert_events(&[usage]).unwrap();
        let reader = StoreReader::open(&path).unwrap();
        let conn = reader.connection();

        let stored = event_pricing(conn, AgentId::CLAUDE, &event.dedup_key)
            .unwrap()
            .unwrap();
        assert_eq!(stored.tokens, event.tokens);
        assert_eq!(stored.estimate, None);
        assert_eq!(
            event_pricing(conn, AgentId::PI, &event.dedup_key).unwrap(),
            None
        );
        assert_eq!(
            earliest_transcript_event(conn, AgentId::CLAUDE, &event.profile).unwrap(),
            Some(event.at)
        );
        assert_eq!(tool_call(conn, AgentId::CLAUDE, "t").unwrap(), None);
        assert_eq!(open_issues(conn).unwrap(), []);
        assert_eq!(last_ingest_skipped(conn, AgentId::CLAUDE).unwrap(), None);
        assert_eq!(unresolved_cwds(conn).unwrap(), Vec::<Utf8PathBuf>::new());
    }
}
