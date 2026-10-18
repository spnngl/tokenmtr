//! Agent collectors of `TokenMtr` (D4, D5, D48): the [`AgentSource`] trait, the [`AgentKind`] enum
//! and the agent-neutral pieces every collector builds on.
//!
//! # Building a collector
//!
//! 1. Implement [`AgentSource`] on a type that owns its [`Profile`]s.
//! 2. Read lines with [`jsonl::read_lines`] and a [`NeedleSet`] built from your
//!    [`line_needles`](AgentSource::line_needles). It returns only complete lines, so
//!    [`ParseOutcome::new_offset`] is the scan's [`jsonl::LineScan::new_offset`].
//! 3. Parse each line into the caller's [`ParseBatch`] with `push_event`, `push_tool_call`,
//!    `push_tool_result` and `push_signal`. The batch deduplicates: events last-wins per dedup
//!    key (D12), tool calls first-wins per tool-use id, signals as sets.
//! 4. Classify every parsed line as a [`LineClass`] and feed it to [`DriftStats::record`]; set
//!    `bytes_read`, `lines_read` and the `no_needle` / `oversized` counters from the scan. Turn
//!    the counters into an issue with [`DriftStats::drift_issue`], passing a [`KeySkeleton`] of
//!    the first unmappable line (D42). Parse issues never contain line content.
//! 5. Add a variant to [`AgentKind`] and an entry to `AgentId::KNOWN` in `tokenmtr-core`.
//!
//! Parsers return no `EventId`: it needs the machine id. [`ParsedEvent::into_usage_event`] and
//! [`ParsedToolCall::into_tool_call`] derive ids in the ingestor.

mod batch;
mod blocks;
pub mod claude;
mod discover;
mod drift;
mod error;
pub mod ingest;
pub mod jsonl;
pub mod pi;
pub mod project;
mod skeleton;
mod source;
#[cfg(test)]
mod testkit;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::tz::TimeZone;
use tokenmtr_core::{AgentId, Capabilities, ParsedEvent};

pub use batch::{BatchParts, ParseBatch, ParsedSignal, ParsedToolCall, ParsedToolResult};
pub use claude::ClaudeSource;
pub use discover::{Discovery, discover};
pub use drift::{
    DRIFT_THRESHOLD_PERCENT, DriftReason, DriftStats, LineClass, ParseIssue, ParseOutcome,
    SkippedLines,
};
pub use error::AgentError;
pub use jsonl::NeedleSet;
pub use pi::PiSource;
pub use skeleton::KeySkeleton;
pub use source::{ByteOffset, Profile, SourceFile};

/// Coarse per-day history an agent can offer (D38).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Aggregates {
    /// `EventKind::DailyAggregate` events.
    pub events: Vec<ParsedEvent>,
    /// Files that did not match the expected shape, with the issue to record against each.
    pub issues: Vec<(Utf8PathBuf, ParseIssue)>,
    /// Files that matched, so an issue recorded against them earlier can be closed.
    pub clean: Vec<Utf8PathBuf>,
}

/// Behaviour every agent implements (D4).
pub trait AgentSource {
    /// The agent's id.
    fn id(&self) -> AgentId;

    /// Drives which widgets the UI shows for this agent.
    fn capabilities(&self) -> Capabilities;

    /// Profiles with their roots, from config and platform defaults. Missing roots are not errors.
    fn profiles(&self) -> &[Profile];

    /// Is this path one of ours? (e.g. `*.jsonl` under a root)
    fn owns(&self, path: &Utf8Path) -> bool;

    /// Parses complete lines from `start`. Pushes events, tool calls and signals into `out`;
    /// returns the new offset, issues and drift stats (D42). A half-written last line is not
    /// consumed.
    fn parse(
        &self,
        file: &SourceFile,
        start: ByteOffset,
        out: &mut ParseBatch,
    ) -> Result<ParseOutcome, AgentError>;

    /// Lines without any of these byte strings are skipped unparsed (D48).
    fn line_needles(&self) -> &'static [&'static [u8]];

    /// Coarse history for days without transcripts (D38), with timestamps at 12:00 in `zone`.
    /// The caller keeps only the days before the profile's earliest transcript event. Default:
    /// none.
    fn aggregates(&self, _profile: &Profile, _zone: &TimeZone) -> Result<Aggregates, AgentError> {
        Ok(Aggregates::default())
    }
}

/// Every agent collector (D4). Dispatch is a `match`, so adding a variant lists every place to
/// handle.
#[derive(Debug, Clone)]
pub enum AgentKind {
    /// Claude Code.
    Claude(ClaudeSource),
    /// pi.
    Pi(PiSource),
}

impl AgentSource for AgentKind {
    fn id(&self) -> AgentId {
        match self {
            Self::Claude(source) => source.id(),
            Self::Pi(source) => source.id(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        match self {
            Self::Claude(source) => source.capabilities(),
            Self::Pi(source) => source.capabilities(),
        }
    }

    fn profiles(&self) -> &[Profile] {
        match self {
            Self::Claude(source) => source.profiles(),
            Self::Pi(source) => source.profiles(),
        }
    }

    fn owns(&self, path: &Utf8Path) -> bool {
        match self {
            Self::Claude(source) => source.owns(path),
            Self::Pi(source) => source.owns(path),
        }
    }

    fn parse(
        &self,
        file: &SourceFile,
        start: ByteOffset,
        out: &mut ParseBatch,
    ) -> Result<ParseOutcome, AgentError> {
        match self {
            Self::Claude(source) => source.parse(file, start, out),
            Self::Pi(source) => source.parse(file, start, out),
        }
    }

    fn line_needles(&self) -> &'static [&'static [u8]] {
        match self {
            Self::Claude(source) => source.line_needles(),
            Self::Pi(source) => source.line_needles(),
        }
    }

    fn aggregates(&self, profile: &Profile, zone: &TimeZone) -> Result<Aggregates, AgentError> {
        match self {
            Self::Claude(source) => source.aggregates(profile, zone),
            Self::Pi(source) => source.aggregates(profile, zone),
        }
    }
}
