//! Format drift (D42): per-scan counters and the issues parsers raise. Agent-neutral.

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use serde::Serialize;

use crate::skeleton::KeySkeleton;
use crate::source::ByteOffset;

/// Skipped usage-bearing lines above this share (in percent) of a scan raise a drift issue.
pub const DRIFT_THRESHOLD_PERCENT: u64 = 1;

/// How a parser classified one line it was handed. Feed it to [`DriftStats::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineClass {
    /// The line produced a usage event.
    Event,
    /// A usage-bearing line deliberately ignored (e.g. Claude's `<synthetic>` model).
    Ignored,
    /// The line parsed but carried no usage event. It may still have produced tool calls or
    /// signals.
    NoUsage,
    /// The line is not valid JSON. `usage_bearing` is whether it mentions usage anyway.
    Malformed {
        /// The raw line contains the `usage` key text.
        usage_bearing: bool,
    },
    /// A known usage-bearing line whose fields could not be mapped.
    Unmapped,
}

/// Lines skipped, by reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SkippedLines {
    /// Dropped by the byte-needle pre-filter, never parsed (D48).
    pub no_needle: u64,
    /// Parsed, but no usage event (see [`LineClass::NoUsage`]).
    pub no_usage: u64,
    /// Usage-bearing but deliberately ignored.
    pub ignored: u64,
    /// Not valid JSON.
    pub malformed: u64,
    /// Usage-bearing but unmappable.
    pub unmapped: u64,
    /// Longer than the reader's line cap, dropped unparsed.
    pub oversized: u64,
}

/// Counters of one parse call. Add several with [`DriftStats::merge`] to cover a whole scan.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct DriftStats {
    /// Bytes consumed: from the start offset to the new offset.
    pub bytes_read: u64,
    /// Complete lines read, filtered or not.
    pub lines_read: u64,
    /// Lines that produced a usage event, before deduplication.
    pub events: u64,
    /// Usage-bearing lines that should have mapped: events plus failures.
    pub usage_lines: u64,
    /// Usage-bearing lines that failed: unmappable lines and malformed lines mentioning usage.
    pub usage_lines_failed: u64,
    /// Lines skipped, by reason.
    pub skipped: SkippedLines,
    /// Agent versions newer than the newest one known (info only, never an issue).
    pub unknown_versions: BTreeSet<String>,
}

impl DriftStats {
    /// Counts one parsed line.
    pub const fn record(&mut self, class: LineClass) {
        match class {
            LineClass::Event => {
                self.events += 1;
                self.usage_lines += 1;
            }
            LineClass::Ignored => self.skipped.ignored += 1,
            LineClass::NoUsage => self.skipped.no_usage += 1,
            LineClass::Malformed { usage_bearing } => {
                self.skipped.malformed += 1;
                if usage_bearing {
                    self.usage_lines += 1;
                    self.usage_lines_failed += 1;
                }
            }
            LineClass::Unmapped => {
                self.skipped.unmapped += 1;
                self.usage_lines += 1;
                self.usage_lines_failed += 1;
            }
        }
    }

    /// Adds the counters of another parse call.
    pub fn merge(&mut self, other: &Self) {
        self.bytes_read += other.bytes_read;
        self.lines_read += other.lines_read;
        self.events += other.events;
        self.usage_lines += other.usage_lines;
        self.usage_lines_failed += other.usage_lines_failed;
        self.skipped.no_needle += other.skipped.no_needle;
        self.skipped.no_usage += other.skipped.no_usage;
        self.skipped.ignored += other.skipped.ignored;
        self.skipped.malformed += other.skipped.malformed;
        self.skipped.unmapped += other.skipped.unmapped;
        self.skipped.oversized += other.skipped.oversized;
        self.unknown_versions
            .extend(other.unknown_versions.iter().cloned());
    }

    /// The drift issue these counters call for, if any (D42): an unmappable usage-bearing line, or
    /// failed usage-bearing lines above [`DRIFT_THRESHOLD_PERCENT`] of the usage-bearing lines.
    /// `sample` is the skeleton of the first unmappable line.
    #[must_use]
    pub fn drift_issue(&self, sample: Option<KeySkeleton>) -> Option<ParseIssue> {
        let reason = if self.skipped.unmapped > 0 {
            DriftReason::UnmappedLine
        } else if self.usage_lines_failed * 100 > self.usage_lines * DRIFT_THRESHOLD_PERCENT {
            DriftReason::SkippedOverThreshold
        } else {
            return None;
        };
        Some(ParseIssue::FormatDrift {
            reason,
            lines: self.usage_lines_failed,
            sample,
        })
    }
}

/// Why a [`ParseIssue::FormatDrift`] was raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftReason {
    /// A known usage-bearing line failed to map.
    UnmappedLine,
    /// Failed usage-bearing lines exceeded the threshold.
    SkippedOverThreshold,
    /// A `stats-cache.json` lacks the expected shape, or some of its day rows do.
    StatsCacheShape,
}

/// A problem worth showing in the UI. Never holds line content (D13, D42).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "issue", rename_all = "snake_case")]
pub enum ParseIssue {
    /// The agent's file format no longer matches the parser. Feeds the `format_drift` alert.
    FormatDrift {
        /// What triggered the issue.
        reason: DriftReason,
        /// Number of failed usage-bearing lines.
        lines: u64,
        /// Skeleton of the first unmappable line, when there was one.
        sample: Option<KeySkeleton>,
    },
    /// A file declares a format version newer than the parser knows (pi header `version` > 3).
    UnknownVersion {
        /// The declared version.
        version: String,
    },
    /// A directory or file could not be listed or read.
    Unreadable {
        /// What failed.
        path: Utf8PathBuf,
        /// The OS error text.
        reason: String,
    },
}

/// What one [`AgentSource::parse`](crate::AgentSource::parse) call reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParseOutcome {
    /// Offset after the last complete line. A half-written last line stays before it (D15).
    pub new_offset: ByteOffset,
    /// Issues raised by this call.
    pub issues: Vec<ParseIssue>,
    /// Counters of this call.
    pub drift: DriftStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(events: u64, malformed_usage: u64) -> DriftStats {
        let mut stats = DriftStats::default();
        for _ in 0..events {
            stats.record(LineClass::Event);
        }
        for _ in 0..malformed_usage {
            stats.record(LineClass::Malformed {
                usage_bearing: true,
            });
        }
        stats
    }

    #[test]
    fn one_percent_is_tolerated_more_is_not() {
        assert_eq!(stats(99, 1).drift_issue(None), None);
        assert!(matches!(
            stats(98, 2).drift_issue(None),
            Some(ParseIssue::FormatDrift {
                reason: DriftReason::SkippedOverThreshold,
                ..
            })
        ));
    }

    #[test]
    fn malformed_lines_without_usage_never_count() {
        let mut stats = stats(1, 0);
        stats.record(LineClass::Malformed {
            usage_bearing: false,
        });
        assert_eq!(stats.drift_issue(None), None);
    }

    #[test]
    fn an_unmappable_line_always_raises() {
        let mut stats = stats(1000, 0);
        stats.record(LineClass::Unmapped);
        assert!(matches!(
            stats.drift_issue(None),
            Some(ParseIssue::FormatDrift {
                reason: DriftReason::UnmappedLine,
                lines: 1,
                ..
            })
        ));
    }
}
