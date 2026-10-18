//! What a scan reports back: counts, skipped lines, drift, issues, throughput.

use std::collections::BTreeMap;
use std::time::Duration;

use camino::Utf8PathBuf;
use serde::Serialize;
use tokenmtr_core::AgentId;
use tokenmtr_store::UpsertStats;

use super::fingerprint::ParseReason;
use crate::drift::DriftStats;

/// One file the scan parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileScan {
    /// The file.
    pub path: Utf8PathBuf,
    /// Why it was parsed.
    pub reason: ParseReason,
    /// First byte read.
    pub from: u64,
    /// Offset after the last complete line; equals `from` when the parse failed.
    pub to: u64,
    /// Usage events the file produced (before deduplication).
    pub events: u64,
    /// Why the parse failed, if it did. The checkpoint did not move.
    pub error: Option<String>,
}

/// Rows written, split by what the store did with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Upserts {
    /// New rows.
    pub inserted: u64,
    /// Rows whose values changed (their revision moved).
    pub updated: u64,
    /// Rows identical to what was stored (nothing moved).
    pub unchanged: u64,
}

impl Upserts {
    pub(crate) fn add(&mut self, stats: &UpsertStats) {
        self.inserted += stats.inserted;
        self.updated += stats.updated;
        self.unchanged += stats.unchanged;
    }

    /// Rows that changed the store: inserted plus updated.
    #[must_use]
    pub const fn changed(&self) -> u64 {
        self.inserted + self.updated
    }
}

/// What the stats-cache import did for an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct AggregateScan {
    /// Aggregate events found in the files.
    pub found: u64,
    /// Dropped because transcripts already cover their day.
    pub covered_by_transcripts: u64,
    /// Written, by outcome.
    pub written: Upserts,
}

/// One agent's part of a scan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentScan {
    /// The agent.
    pub agent: AgentId,
    /// Files found by discovery (or named by the caller).
    pub files_discovered: u64,
    /// Files parsed.
    pub files_parsed: u64,
    /// Files whose checkpoint matched: not read at all.
    pub files_unchanged: u64,
    /// Files that could not be parsed.
    pub files_failed: u64,
    /// Bytes consumed by the parses.
    pub bytes_parsed: u64,
    /// Parse counters, summed over the files.
    pub drift: DriftStats,
    /// Usage events written.
    pub events: Upserts,
    /// Tool calls written.
    pub tool_calls: Upserts,
    /// New session signals.
    pub signals_new: u64,
    /// Tool results whose call was stored by an earlier scan and got its error flag now.
    pub orphan_results_applied: u64,
    /// Tool results whose call is not stored (yet).
    pub orphan_results_unmatched: u64,
    /// Parse time summed over the files: CPU time, not wall time, when files parse in parallel.
    pub parse_time: Duration,
    /// Stats-cache import, when it ran.
    pub aggregates: Option<AggregateScan>,
    /// The files parsed, in completion order.
    pub files: Vec<FileScan>,
}

impl AgentScan {
    pub(crate) fn new(agent: AgentId) -> Self {
        Self {
            agent,
            files_discovered: 0,
            files_parsed: 0,
            files_unchanged: 0,
            files_failed: 0,
            bytes_parsed: 0,
            drift: DriftStats::default(),
            events: Upserts::default(),
            tool_calls: Upserts::default(),
            signals_new: 0,
            orphan_results_applied: 0,
            orphan_results_unmatched: 0,
            parse_time: Duration::ZERO,
            aggregates: None,
            files: Vec::new(),
        }
    }

    /// Parse throughput in MB/s (10^6 bytes) per core, `None` before anything was parsed.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a throughput figure for display; precision beyond 2^53 bytes is irrelevant"
    )]
    pub fn megabytes_per_second(&self) -> Option<f64> {
        let seconds = self.parse_time.as_secs_f64();
        (seconds > 0.0).then(|| self.bytes_parsed as f64 / 1e6 / seconds)
    }

    /// The counters stored with the ingest run: skipped lines by reason plus the usage-line
    /// totals the drift check divides by.
    #[must_use]
    pub fn skipped_counters(&self) -> BTreeMap<String, u64> {
        let skipped = &self.drift.skipped;
        [
            ("no_needle", skipped.no_needle),
            ("no_usage", skipped.no_usage),
            ("ignored", skipped.ignored),
            ("malformed", skipped.malformed),
            ("unmapped", skipped.unmapped),
            ("oversized", skipped.oversized),
            ("usage_lines", self.drift.usage_lines),
            ("usage_lines_failed", self.drift.usage_lines_failed),
        ]
        .into_iter()
        .map(|(name, count)| (name.to_owned(), count))
        .collect()
    }
}

/// The result of one [`Ingestor::scan`](super::Ingestor::scan).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanReport {
    /// One entry per scanned agent, in settings order.
    pub agents: Vec<AgentScan>,
    /// Working directories whose project changed because a `[[projects]]` rule changed.
    pub cwds_reresolved: u64,
    /// Stored events that moved to another project or client because of that.
    pub events_reassigned: u64,
    /// Wall time of the whole scan.
    pub elapsed: Duration,
}

impl ScanReport {
    /// Bytes parsed over all agents.
    #[must_use]
    pub fn bytes_parsed(&self) -> u64 {
        self.agents.iter().map(|a| a.bytes_parsed).sum()
    }

    /// Usage events inserted or updated over all agents.
    #[must_use]
    pub fn events_changed(&self) -> u64 {
        self.agents.iter().map(|a| a.events.changed()).sum()
    }

    /// The part of the agent `agent`, if it was scanned.
    #[must_use]
    pub fn agent(&self, agent: AgentId) -> Option<&AgentScan> {
        self.agents.iter().find(|a| a.agent == agent)
    }
}
