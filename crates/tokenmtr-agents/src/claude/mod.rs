//! Claude Code collector (§5): profiles, transcript discovery and parsing.

mod line;
pub mod stats_cache;

use camino::Utf8Path;
use jiff::tz::TimeZone;
use tokenmtr_core::{AgentId, Capabilities, ProfileId};

use crate::batch::ParseBatch;
use crate::drift::{DriftReason, ParseIssue};
use crate::drift::{DriftStats, LineClass, ParseOutcome};
use crate::error::AgentError;
use crate::jsonl::{self, NeedleSet};
use crate::skeleton::KeySkeleton;
use crate::source::{ByteOffset, Profile, SourceFile};
use crate::{AgentSource, Aggregates};

pub use line::{LineContext, LineReport, parse_line};

/// Lines without one of these are skipped unparsed (D48).
const LINE_NEEDLES: &[&[u8]] = &[
    b"usage",
    b"tool_use",
    b"tool_result",
    b"isApiErrorMessage",
    b"isCompactSummary",
];

/// The newest Claude Code `version` present in `fixtures/claude`. A line declaring a newer one is
/// recorded in [`DriftStats::unknown_versions`] (D42, info only).
pub const NEWEST_KNOWN_VERSION: &str = "2.1.287";

/// The Claude Code collector.
#[derive(Debug, Clone)]
pub struct ClaudeSource {
    profiles: Vec<Profile>,
    needles: NeedleSet,
}

impl ClaudeSource {
    /// A collector over `profiles`. Build them with [`default_profile`] and
    /// [`profile_for_config_dir`].
    #[must_use]
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self {
            profiles,
            needles: NeedleSet::new(LINE_NEEDLES),
        }
    }

    /// Turns the line pre-filter off: every line is parsed. The output must not change (D48); used
    /// by the equivalence tests and by benchmarks that compare both modes.
    #[must_use]
    pub fn without_prefilter(mut self) -> Self {
        self.needles = NeedleSet::all();
        self
    }
}

/// The `default` profile: `$CLAUDE_CONFIG_DIR/projects` (when set), `~/.claude/projects` and
/// `~/.config/claude/projects`. The caller reads the environment; this stays pure.
#[must_use]
pub fn default_profile(home: &Utf8Path, claude_config_dir: Option<&Utf8Path>) -> Profile {
    let roots = claude_config_dir
        .map(|dir| dir.join("projects"))
        .into_iter()
        .chain([
            home.join(".claude").join("projects"),
            home.join(".config").join("claude").join("projects"),
        ])
        .collect();
    Profile::new(ProfileId::default_profile(), roots)
}

/// A profile whose Claude config dir is `config_dir` (used through `CLAUDE_CONFIG_DIR`): the one
/// root is `<config_dir>/projects`.
#[must_use]
pub fn profile_for_config_dir(id: ProfileId, config_dir: &Utf8Path) -> Profile {
    Profile::new(id, vec![config_dir.join("projects")])
}

impl AgentSource for ClaudeSource {
    fn id(&self) -> AgentId {
        AgentId::CLAUDE
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            quota: true,
            reported_cost: false,
            subscription: true,
        }
    }

    fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    fn owns(&self, path: &Utf8Path) -> bool {
        path.extension() == Some("jsonl") && self.profiles.iter().any(|p| p.contains(path))
    }

    fn parse(
        &self,
        file: &SourceFile,
        start: ByteOffset,
        out: &mut ParseBatch,
    ) -> Result<ParseOutcome, AgentError> {
        let ctx = LineContext::new(file)?;
        let mut stats = DriftStats::default();
        let mut sample = None;
        let mut last_version = String::new();
        let scan = jsonl::read_lines(&file.path, start, &self.needles, |bytes| {
            let report = parse_line(&ctx, bytes, out);
            stats.record(report.class);
            if report.class == LineClass::Unmapped && sample.is_none() {
                sample = Some(KeySkeleton::from_json(bytes));
            }
            if let Some(version) = report.version
                && *version != *last_version
            {
                last_version.clear();
                last_version.push_str(&version);
                if is_newer(&version, NEWEST_KNOWN_VERSION) {
                    stats.unknown_versions.insert(version.into_owned());
                }
            }
        })?;
        stats.record_scan(start, &scan);
        Ok(ParseOutcome {
            new_offset: scan.new_offset,
            last_line_hash: scan.last_line_hash,
            issues: stats.unmapped_issue(sample).into_iter().collect(),
            drift: stats,
        })
    }

    fn line_needles(&self) -> &'static [&'static [u8]] {
        LINE_NEEDLES
    }

    fn aggregates(&self, profile: &Profile, zone: &TimeZone) -> Result<Aggregates, AgentError> {
        let mut out = Aggregates::default();
        for dir in config_dirs(profile) {
            let path = dir.join(STATS_CACHE_FILE);
            let body = match std::fs::read(&path) {
                Ok(body) => body,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    let reason = err.to_string();
                    out.issues
                        .push((path.clone(), ParseIssue::Unreadable { path, reason }));
                    continue;
                }
            };
            let cache = stats_cache::parse(&body, &profile.id, zone);
            if !cache.recognised || cache.bad_rows > 0 {
                let lines = cache.bad_rows.max(1);
                out.issues.push((
                    path,
                    ParseIssue::FormatDrift {
                        reason: DriftReason::StatsCacheShape,
                        lines,
                        sample: None,
                    },
                ));
            } else {
                out.clean.push(path);
            }
            out.events.extend(cache.events);
        }
        Ok(out)
    }
}

/// File name of Claude's lifetime statistics, inside the config dir.
const STATS_CACHE_FILE: &str = "stats-cache.json";

/// Claude config dirs of `profile`: the parent of every root named `projects`.
pub fn config_dirs(profile: &Profile) -> impl Iterator<Item = &Utf8Path> {
    profile
        .roots
        .iter()
        .filter(|root| root.file_name() == Some("projects"))
        .filter_map(|root| root.parent())
}

/// Whether dotted numeric version `version` is newer than `known`. Text that is not dotted
/// numbers (a pre-release tag, garbage) is never newer.
fn is_newer(version: &str, known: &str) -> bool {
    fn parts(text: &str) -> Option<Vec<u64>> {
        text.split('.').map(|part| part.parse().ok()).collect()
    }
    matches!((parts(version), parts(known)), (Some(a), Some(b)) if a > b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_compare_numerically() {
        assert!(is_newer("2.1.1", "2.1.0"));
        assert!(is_newer("2.10.0", "2.9.0"));
        assert!(!is_newer("2.1.0", "2.1.0"));
        assert!(!is_newer("2.0.9", "2.1.0"));
        assert!(!is_newer("3.0.0-beta", "2.1.0"));
        assert!(!is_newer("", "2.1.0"));
    }

    #[test]
    fn default_profile_roots_follow_the_plan() {
        let profile = default_profile(
            Utf8Path::new("/home/u"),
            Some(Utf8Path::new("/cfg/claude-work")),
        );
        let roots: Vec<&str> = profile.roots.iter().map(|root| root.as_str()).collect();
        assert_eq!(
            roots,
            [
                "/cfg/claude-work/projects",
                "/home/u/.claude/projects",
                "/home/u/.config/claude/projects"
            ]
        );
    }

    #[test]
    fn owns_only_jsonl_under_a_root() {
        let source = ClaudeSource::new(vec![profile_for_config_dir(
            ProfileId::default_profile(),
            Utf8Path::new("/c"),
        )]);
        assert!(source.owns(Utf8Path::new("/c/projects/p/s.jsonl")));
        assert!(source.owns(Utf8Path::new("/c/projects/p/s/subagents/agent-1.jsonl")));
        assert!(!source.owns(Utf8Path::new("/c/projects/p/s.json")));
        assert!(!source.owns(Utf8Path::new("/elsewhere/s.jsonl")));
    }
}
