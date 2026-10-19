//! What one output sees: its exclusions and redaction applied to the shared [`Snapshot`].
//!
//! Matching uses raw values, before redaction. A group whose cwd matches `exclude_paths`, or
//! whose project key, name or git remote matches `exclude_projects`, is dropped. A group with
//! neither is never excluded. Redaction then rewrites label values (`hash` is
//! [`RedactionKey::hash`], what the sinks write; `drop` is the empty string) and series that
//! became equal are summed, keeping the earliest `created`.

use std::collections::BTreeMap;

use jiff::Timestamp;
use tokenmtr_config::{GlobList, OutputFilter, RedactColumn, RedactMode};
use tokenmtr_store::RedactionKey;

use crate::catalog::{Family, TOKEN_TYPES};
use crate::snapshot::{Origin, QuotaGauge, Snapshot, ToolGroup, UsageGroup};

/// The workspace version, the value of `tokenmtr_build_info{version}`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One output's filter plus the key its hashes use.
#[derive(Debug, Clone)]
pub struct OutputScope {
    exclude_projects: GlobList,
    exclude_paths: GlobList,
    redact: BTreeMap<RedactColumn, RedactMode>,
    key: RedactionKey,
}

impl OutputScope {
    /// `key` is the store's redaction key ([`tokenmtr_store::StoreRead::redaction_key`]).
    #[must_use]
    pub fn new(filter: &OutputFilter, key: &RedactionKey) -> Self {
        Self {
            exclude_projects: filter.exclude_projects.clone(),
            exclude_paths: filter.exclude_paths.clone(),
            redact: filter.redact.clone(),
            key: key.clone(),
        }
    }

    fn excludes(&self, origin: &Origin) -> bool {
        origin
            .cwd
            .as_deref()
            .is_some_and(|cwd| self.exclude_paths.is_match(cwd))
            || [&origin.project, &origin.name, &origin.remote]
                .into_iter()
                .flatten()
                .any(|p| self.exclude_projects.is_match(p))
    }

    /// The value of label `name` after redaction. Empty stays empty: there is nothing to hide.
    fn redacted(&self, name: &str, value: &str) -> String {
        let column = match name {
            "project" => RedactColumn::Project,
            "client" => RedactColumn::Client,
            "git_branch" => RedactColumn::GitBranch,
            "account" => RedactColumn::Account,
            "session_id" => RedactColumn::SessionId,
            _ => return value.to_owned(),
        };
        match self.redact.get(&column) {
            _ if value.is_empty() => String::new(),
            Some(RedactMode::Hash) => self.key.hash(value),
            Some(RedactMode::Drop) => String::new(),
            None => value.to_owned(),
        }
    }
}

/// A sample's value; the family's [`crate::Kind`] says which one it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// A counter.
    Count(u64),
    /// A gauge.
    Gauge(f64),
}

/// One series of one output.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// The family.
    pub family: Family,
    /// Label names and values, sorted by name; the names are the family's.
    pub labels: Vec<(&'static str, String)>,
    /// The value.
    pub value: Value,
    /// The earliest row behind a counter (P21); `None` for gauges.
    pub created: Option<Timestamp>,
}

/// An output's view of one snapshot: its series, sorted by family then labels, so the encoded
/// body is deterministic.
#[derive(Debug, Clone, PartialEq)]
pub struct Folded {
    /// When the snapshot was computed.
    pub taken_at: Timestamp,
    /// Every series, meta series included.
    pub series: Vec<Series>,
}

/// A label source of one group: its value for a catalog label name.
trait Labelled {
    fn label(&self, name: &str) -> &str;
}

impl Labelled for UsageGroup {
    fn label(&self, name: &str) -> &str {
        match name {
            "account" => &self.account,
            "agent" => &self.agent,
            "billing" => self.billing,
            "client" => &self.client,
            "git_branch" => self.session.as_ref().map_or("", |(_, branch)| branch),
            "model" => &self.model,
            "profile" => &self.profile,
            "project" => &self.project,
            "provider" => &self.provider,
            "session_id" => self.session.as_ref().map_or("", |(id, _)| id),
            _ => "",
        }
    }
}

impl Labelled for ToolGroup {
    fn label(&self, name: &str) -> &str {
        match name {
            "agent" => &self.agent,
            "error" => self.error,
            "mcp_server" => &self.mcp_server,
            "profile" => &self.profile,
            "session_id" => self.session.as_deref().unwrap_or(""),
            "tool" => &self.tool,
            _ => "",
        }
    }
}

impl Labelled for QuotaGauge {
    fn label(&self, name: &str) -> &str {
        match name {
            "account" => &self.account,
            "agent" => &self.agent,
            "profile" => &self.profile,
            "window" => &self.window,
            _ => "",
        }
    }
}

type Key = (Family, Vec<String>);

/// Series under construction: counters add up, gauges overwrite.
#[derive(Default)]
struct Accumulator {
    series: BTreeMap<Key, (Value, Option<i64>)>,
}

impl Accumulator {
    fn count(&mut self, key: Key, add: u64, created_us: i64) {
        let slot = self
            .series
            .entry(key)
            .or_insert((Value::Count(0), Some(created_us)));
        if let Value::Count(total) = &mut slot.0 {
            *total = total.saturating_add(add);
        }
        slot.1 = slot.1.map(|at| at.min(created_us)).or(Some(created_us));
    }

    fn gauge(&mut self, key: Key, value: f64) {
        self.series.insert(key, (Value::Gauge(value), None));
    }
}

impl Snapshot {
    /// This snapshot as `scope`'s output sees it, plus the meta series (`tokenmtr_series`,
    /// `tokenmtr_snapshot_timestamp_seconds`, `tokenmtr_build_info`).
    #[must_use]
    pub fn fold(&self, scope: &OutputScope) -> Folded {
        let excluded: Vec<bool> = self.origins.iter().map(|o| scope.excludes(o)).collect();
        let mut acc = Accumulator::default();
        let key = |family: Family, source: &dyn Labelled, extra: Option<&str>| -> Key {
            let values = family
                .labels()
                .iter()
                .map(|name| {
                    let value = match (*name, extra) {
                        ("type" | "kind", Some(extra)) => extra,
                        _ => source.label(name),
                    };
                    scope.redacted(name, value)
                })
                .collect();
            (family, values)
        };

        for (groups, tokens, reasoning, events) in [
            (
                &self.usage,
                Family::Tokens,
                Family::ReasoningTokens,
                Family::Events,
            ),
            (
                &self.session_usage,
                Family::SessionTokens,
                Family::SessionReasoningTokens,
                Family::SessionEvents,
            ),
        ] {
            for group in groups.iter().filter(|g| !excluded[g.origin]) {
                for (count, name) in group.tokens.iter().zip(TOKEN_TYPES) {
                    if *count > 0 {
                        acc.count(key(tokens, group, Some(name)), *count, group.created_us);
                    }
                }
                if group.reasoning > 0 {
                    acc.count(
                        key(reasoning, group, None),
                        group.reasoning,
                        group.created_us,
                    );
                }
                acc.count(
                    key(events, group, Some(&group.kind)),
                    group.events,
                    group.created_us,
                );
            }
        }
        for (groups, family) in [
            (&self.tools, Family::ToolCalls),
            (&self.session_tools, Family::SessionToolCalls),
        ] {
            for group in groups.iter().filter(|g| !excluded[g.origin]) {
                acc.count(key(family, group, None), group.calls, group.created_us);
            }
        }
        for gauge in &self.quota {
            if let Some(ratio) = gauge.used_ratio {
                acc.gauge(key(Family::QuotaUsedRatio, gauge, None), ratio);
            }
            if let Some(us) = gauge.resets_at_us {
                acc.gauge(key(Family::QuotaResetsAt, gauge, None), seconds(us));
            }
            acc.gauge(
                key(Family::QuotaFetchedAt, gauge, Some("")),
                seconds(gauge.fetched_at_us),
            );
        }

        let series = acc.series.len() + 3;
        acc.gauge((Family::Series, Vec::new()), usize_gauge(series));
        acc.gauge(
            (Family::SnapshotTimestamp, Vec::new()),
            seconds(self.taken_at.as_microsecond()),
        );
        acc.gauge((Family::BuildInfo, vec![VERSION.to_owned()]), 1.0);

        Folded {
            taken_at: self.taken_at,
            series: acc
                .series
                .into_iter()
                .map(|((family, values), (value, created_us))| Series {
                    family,
                    labels: family.labels().iter().copied().zip(values).collect(),
                    value,
                    created: created_us.and_then(|us| Timestamp::from_microsecond(us).ok()),
                })
                .collect(),
        }
    }
}

/// Unix seconds, with microsecond resolution, as a gauge value.
#[expect(
    clippy::cast_precision_loss,
    reason = "a timestamp in microseconds is far below 2^53"
)]
const fn seconds(us: i64) -> f64 {
    us as f64 / 1_000_000.0
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a series count is far below 2^53"
)]
const fn usize_gauge(n: usize) -> f64 {
    n as f64
}
