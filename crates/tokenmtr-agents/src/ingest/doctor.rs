//! The `doctor` report: what the ingestor sees on this machine (Phase 6, D50, D53).

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use serde::Serialize;
use tokenmtr_core::{AgentId, ProfileId};
use tokenmtr_store::{OpenParseIssue, StoreRead};

use super::engine::Ingestor;
use super::error::IngestError;
use super::fingerprint::FileStat;
use crate::{AgentKind, AgentSource as _, claude, discover};

/// Days Claude keeps transcripts when `settings.json` does not say (D53).
const DEFAULT_CLEANUP_PERIOD_DAYS: u64 = 30;

/// One root directory of a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootReport {
    /// The directory.
    pub path: Utf8PathBuf,
    /// Whether it exists.
    pub exists: bool,
    /// Source files under it.
    pub files: u64,
    /// Their total size in bytes.
    pub bytes: u64,
}

/// How long Claude keeps transcripts (D53): the retention that bounds the history we can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Retention {
    /// `cleanupPeriodDays` from `settings.json`, or the default of 30.
    pub cleanup_period_days: u64,
    /// Whether `settings.json` set it (otherwise the default applies).
    pub configured: bool,
    /// Modification time of the oldest transcript on disk.
    pub oldest_transcript: Option<Timestamp>,
    /// Whole days since then.
    pub oldest_age_days: Option<u64>,
    /// Days until Claude deletes that transcript; 0 when it is due.
    pub days_until_oldest_expires: Option<u64>,
}

/// One profile of an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileReport {
    /// The profile.
    pub profile: ProfileId,
    /// Its roots.
    pub roots: Vec<RootReport>,
    /// Claude's transcript retention, for Claude profiles with a config dir.
    pub retention: Option<Retention>,
}

/// The latest ingest run that parsed something.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LastRun {
    /// When it finished.
    pub finished: Timestamp,
    /// Skipped-line counters it stored.
    pub skipped: BTreeMap<String, u64>,
    /// Usage lines that failed to map, in percent of usage lines; `None` without usage lines.
    pub failed_usage_percent: Option<f64>,
}

/// One open parse issue, as the doctor lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

impl From<OpenParseIssue> for OpenIssue {
    fn from(issue: OpenParseIssue) -> Self {
        Self {
            agent: issue.agent.as_str().to_owned(),
            kind: issue.kind.as_str().to_owned(),
            subject: issue.subject,
            occurrences: issue.occurrences,
            sample: issue.sample,
        }
    }
}

/// One agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentReport {
    /// The agent.
    pub agent: AgentId,
    /// Its profiles.
    pub profiles: Vec<ProfileReport>,
    /// Files with a stored checkpoint.
    pub checkpoints: u64,
    /// Latest modification time among the checkpointed files.
    pub newest_checkpointed_mtime: Option<Timestamp>,
    /// Latest ingest run.
    pub last_run: Option<LastRun>,
}

/// Everything `doctor` prints.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DoctorReport {
    /// Per agent.
    pub agents: Vec<AgentReport>,
    /// Open parse issues of every agent.
    pub open_issues: Vec<OpenIssue>,
    /// Working directories of stored events that have no project resolution yet.
    pub unresolved_cwds: Vec<Utf8PathBuf>,
    /// Working directories under a macOS protected folder (D50).
    pub protected_cwds: Vec<Utf8PathBuf>,
    /// Sink directories under a macOS protected folder (D50).
    pub protected_sinks: Vec<Utf8PathBuf>,
}

impl Ingestor {
    /// Inspects roots, checkpoints, issues, projects and retention. Reads only. `sink_dirs` are
    /// the directories of the configured file sinks, checked against the protected folders.
    pub fn doctor(
        &self,
        store: &impl StoreRead,
        sink_dirs: &[Utf8PathBuf],
    ) -> Result<DoctorReport, IngestError> {
        let now = Timestamp::now();
        let mut agents = Vec::new();
        for source in &self.settings.sources {
            let agent = source.id();
            let checkpoints = store.load_checkpoints(agent)?;
            let last_run = store
                .ingest_runs(Some(agent), 1)?
                .into_iter()
                .next()
                .map(|run| {
                    let total = run.skipped.get("usage_lines").copied().unwrap_or(0);
                    let failed = run.skipped.get("usage_lines_failed").copied().unwrap_or(0);
                    LastRun {
                        finished: run.finished,
                        failed_usage_percent: (total > 0).then(|| percent(failed, total)),
                        skipped: run.skipped,
                    }
                });
            agents.push(AgentReport {
                agent,
                profiles: Self::profile_reports(source, now),
                checkpoints: u64::try_from(checkpoints.len()).unwrap_or(u64::MAX),
                newest_checkpointed_mtime: checkpoints.iter().map(|c| c.checkpoint.mtime).max(),
                last_run,
            });
        }
        let protected_cwds = store
            .projects()?
            .into_iter()
            .map(|resolution| resolution.cwd)
            .filter(|cwd| self.under_protected(cwd))
            .collect();
        Ok(DoctorReport {
            agents,
            open_issues: store
                .open_parse_issues()?
                .into_iter()
                .map(OpenIssue::from)
                .collect(),
            unresolved_cwds: store.unresolved_cwds()?,
            protected_cwds,
            protected_sinks: sink_dirs
                .iter()
                .filter(|dir| self.under_protected(dir))
                .cloned()
                .collect(),
        })
    }

    fn under_protected(&self, path: &Utf8Path) -> bool {
        self.settings
            .protected
            .iter()
            .any(|folder| tokenmtr_platform::paths::is_under(path, folder))
    }

    fn profile_reports(source: &AgentKind, now: Timestamp) -> Vec<ProfileReport> {
        let found = discover(source);
        source
            .profiles()
            .iter()
            .map(|profile| {
                let mut oldest: Option<Timestamp> = None;
                let roots = profile
                    .roots
                    .iter()
                    .map(|root| {
                        let mut report = RootReport {
                            path: root.clone(),
                            exists: root.is_dir(),
                            files: 0,
                            bytes: 0,
                        };
                        for file in found
                            .files
                            .iter()
                            .filter(|f| f.profile == profile.id && f.path.starts_with(root))
                        {
                            report.files += 1;
                            if let Ok(stat) = FileStat::of(&file.path) {
                                report.bytes += stat.size;
                                oldest = Some(oldest.map_or(stat.mtime, |o| o.min(stat.mtime)));
                            }
                        }
                        report
                    })
                    .collect();
                let retention = match source {
                    AgentKind::Claude(_) => claude::config_dirs(profile)
                        .next()
                        .map(|dir| retention(dir, oldest, now)),
                    AgentKind::Pi(_) => None,
                };
                ProfileReport {
                    profile: profile.id.clone(),
                    roots,
                    retention,
                }
            })
            .collect()
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a percentage for display; counts beyond 2^53 are irrelevant"
)]
fn percent(part: u64, total: u64) -> f64 {
    part as f64 * 100.0 / total as f64
}

fn retention(config_dir: &Utf8Path, oldest: Option<Timestamp>, now: Timestamp) -> Retention {
    let configured = std::fs::read(config_dir.join("settings.json"))
        .ok()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
        .and_then(|settings| settings.get("cleanupPeriodDays")?.as_u64());
    let cleanup_period_days = configured.unwrap_or(DEFAULT_CLEANUP_PERIOD_DAYS);
    let age_days = oldest.map(|at| {
        let seconds = now.as_second().saturating_sub(at.as_second()).max(0);
        u64::try_from(seconds / 86_400).unwrap_or(0)
    });
    Retention {
        cleanup_period_days,
        configured: configured.is_some(),
        oldest_transcript: oldest,
        oldest_age_days: age_days,
        days_until_oldest_expires: age_days.map(|age| cleanup_period_days.saturating_sub(age)),
    }
}
