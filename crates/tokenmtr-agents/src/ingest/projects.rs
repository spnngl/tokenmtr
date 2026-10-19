//! The `project_map` cache and re-resolution after a rule change.
//!
//! A cwd is resolved once and stored. Changing a `[[projects]]` rule or `[git] default_remote`
//! re-resolves the cwds whose stored answer no longer holds and rewrites the project and client of their events; the store
//! bumps a revision only on rows that really change, so sinks re-export just those.

use std::borrow::Cow;
use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::{SignedDuration, Timestamp};
use tokenmtr_core::{AgentId, ProjectKey, UsageEvent};
use tokenmtr_store::{
    IssueKind, IssueUpdate, ParseIssue, ProjectResolution, ProjectSource, RedactionKey, Store,
    StoreRead as _,
};

use super::error::IngestError;
use crate::project::superset::Hints;
use crate::project::{CwdProblem, ProjectResolver};

/// Subject of the `integration_drift` issue about Superset's database.
const SUPERSET_SUBJECT: &str = "superset";

/// A cwd's project and client, as stored on events.
pub type Assignment = (ProjectKey, Option<String>);

/// Which stored resolutions [`ProjectCache::reresolve`] re-resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reresolve {
    /// Those a rule change invalidated.
    Stale,
    /// Those, and every cwd that fell back to itself: a cwd that was missing or refused may be
    /// readable now. Once per process, so a daemon restart retries them.
    StaleAndFallbacks,
    /// Every one (`scan --full`, "Retry").
    All,
}

/// Resolutions loaded from the store, extended as new cwds appear. Lives for one scan.
pub struct ProjectCache {
    map: HashMap<Utf8PathBuf, ProjectResolution>,
    /// Set under `privacy.redact_paths`: the key that replaces every stored path.
    redaction: Option<RedactionKey>,
    /// Hints for cwds git cannot inspect, read on first use.
    hints: Hints,
    /// The agents already given this scan's `integration_drift` issue.
    drift_reported: Vec<AgentId>,
    /// See [`BranchStamp`].
    branches: BranchStamp,
}

/// How an event whose log names no git branch gets one ([`ProjectCache::branch`]):
///
/// 1. The branch checked out in its cwd now, when the event is at most `live_within` old. A
///    worktree's HEAD can change between scans, so only events of the last two scan intervals
///    are trusted to belong to the current HEAD. Read once per cwd per scan.
/// 2. Else the workspace's last branch from the hints: the branch Superset recorded for the
///    worktree, true for a deleted worktree, possibly stale for a live one.
pub struct BranchStamp {
    /// The age up to which an event takes its cwd's current HEAD.
    pub live_within: SignedDuration,
    heads: HashMap<Utf8PathBuf, Option<String>>,
}

impl BranchStamp {
    /// Trusts the current HEAD for events at most `live_within` old.
    #[must_use]
    pub fn new(live_within: SignedDuration) -> Self {
        Self {
            live_within,
            heads: HashMap::new(),
        }
    }
}

impl ProjectCache {
    /// Loads every stored resolution. With `redaction`, the cache is keyed by hashed cwds and
    /// nothing path-like reaches the store.
    pub fn load(
        store: &Store,
        redaction: Option<RedactionKey>,
        hints: Hints,
        branches: BranchStamp,
    ) -> Result<Self, IngestError> {
        let map = store
            .projects()?
            .into_iter()
            .map(|resolution| (resolution.cwd.clone(), resolution))
            .collect();
        Ok(Self {
            map,
            redaction,
            hints,
            drift_reported: Vec::new(),
            branches,
        })
    }

    /// The git branch of an event at `at` in `cwd` whose log names none, per [`BranchStamp`].
    pub fn branch(
        &mut self,
        resolver: &ProjectResolver,
        cwd: &Utf8Path,
        at: Timestamp,
        now: Timestamp,
    ) -> Option<String> {
        if now.duration_since(at) <= self.branches.live_within {
            let head = self
                .branches
                .heads
                .entry(cwd.to_owned())
                .or_insert_with(|| resolver.head_branch(cwd));
            if head.is_some() {
                return head.clone();
            }
        }
        self.hints.for_cwd(cwd)?.branch.clone()
    }

    /// Closes the `integration_drift` issues of `agents` when the hints were read with the
    /// known schema this scan.
    pub fn close_drift(
        &self,
        store: &mut Store,
        agents: &[AgentId],
        now: Timestamp,
    ) -> Result<(), IngestError> {
        if self.hints.read_ok() {
            for agent in agents {
                store.close_parse_issue(
                    *agent,
                    IssueKind::IntegrationDrift,
                    SUPERSET_SUBJECT,
                    now,
                )?;
            }
        }
        Ok(())
    }

    /// One `integration_drift` issue per agent per scan while the hints are off for drift.
    fn drift_issue(&mut self, agent: AgentId, now: Timestamp) -> Option<ParseIssue> {
        let message = self.hints.drift()?;
        if self.drift_reported.contains(&agent) {
            return None;
        }
        self.drift_reported.push(agent);
        Some(ParseIssue {
            agent,
            kind: IssueKind::IntegrationDrift,
            subject: SUPERSET_SUBJECT.to_owned(),
            count: std::num::NonZeroU64::MIN,
            sample: Some(format!(
                "Superset's database has an unknown shape; deleted worktrees keep their folder name ({message})"
            )),
            at: now,
        })
    }

    /// What the store holds for `cwd`: the cwd, or its keyed hash under redaction.
    fn stored_cwd<'a>(&self, cwd: &'a Utf8Path) -> Cow<'a, Utf8Path> {
        match &self.redaction {
            Some(key) => Cow::Owned(Utf8PathBuf::from(key.hash(cwd.as_str()))),
            None => Cow::Borrowed(cwd),
        }
    }

    /// Sets the project and client of `event` from `assignment`, then replaces its cwd and git
    /// branch by their keyed hashes under redaction.
    pub fn label(&self, event: &mut UsageEvent, assignment: Option<Assignment>) {
        if let Some((project, client)) = assignment {
            event.project = Some(project);
            event.client = client;
        }
        if let Some(key) = &self.redaction {
            event.cwd = event
                .cwd
                .take()
                .map(|cwd| Utf8PathBuf::from(key.hash(cwd.as_str())));
            event.git_branch = event.git_branch.take().map(|branch| key.hash(&branch));
        }
    }

    /// The project and client of `cwd`: from the cache, else resolved now and stored. A cwd that
    /// cannot be inspected adds one issue (`missing_cwd` or `protected_folder`) to `issues`,
    /// which the caller commits with the events, only the first time it is seen. `None` for a
    /// blank cwd.
    pub fn assign(
        &mut self,
        store: &mut Store,
        resolver: &ProjectResolver,
        agent: AgentId,
        cwd: &Utf8Path,
        now: Timestamp,
        issues: &mut Vec<IssueUpdate>,
    ) -> Result<Option<Assignment>, IngestError> {
        let stored_cwd = self.stored_cwd(cwd);
        if let Some(known) = self.map.get(&*stored_cwd) {
            return Ok(Some((known.project.clone(), known.client.clone())));
        }
        // Resolve from the real cwd, then hide it: rules and git need the real path.
        let found = resolver.resolve(cwd, now, &mut self.hints);
        if let Some(issue) = self.drift_issue(agent, now) {
            issues.push(IssueUpdate::Record(issue));
        }
        let Some(found) = found else {
            return Ok(None);
        };
        let Some(resolution) = self.redacted(found.resolution) else {
            return Ok(None);
        };
        // Saved at once, outside the event transaction: a cached resolution without events is
        // harmless, and a fallback is retried on the next start (`Reresolve::StaleAndFallbacks`).
        store.save_project(&resolution)?;
        if let Some(problem) = found.problem {
            issues.push(IssueUpdate::Record(cwd_issue(
                agent,
                &stored_cwd,
                problem,
                now,
            )));
        }
        let assignment = (resolution.project.clone(), resolution.client.clone());
        self.map.insert(stored_cwd.into_owned(), resolution);
        Ok(Some(assignment))
    }

    /// `resolution` with every path replaced by its keyed hash, when redacting. The display
    /// name stays (it is the project's name); the repository root is dropped, and a project key
    /// that is a path (no remote, no rule) is hashed. `None` only if the hash were blank.
    fn redacted(&self, mut resolution: ProjectResolution) -> Option<ProjectResolution> {
        let Some(key) = &self.redaction else {
            return Some(resolution);
        };
        resolution.cwd = Utf8PathBuf::from(key.hash(resolution.cwd.as_str()));
        // Remotes are stored by repository root, which is a path: none are kept.
        resolution.repo_root = None;
        resolution.remotes.clear();
        let path_key = match resolution.source {
            ProjectSource::RepoRoot | ProjectSource::Cwd => true,
            ProjectSource::Inferred => resolution.remote.is_none(),
            ProjectSource::Rule(_) | ProjectSource::Remote => false,
        };
        if path_key {
            resolution.project =
                ProjectKey::try_from(key.hash(resolution.project.as_str())).ok()?;
        }
        Some(resolution)
    }

    /// Re-resolves the cwds `scope` selects, saves the new answers and rewrites the events of
    /// cwds whose project or client moved. A fallback cwd that is readable or inferred now gets
    /// its `missing_cwd` and `protected_folder` issues of `agents` closed, and an inferred cwd's
    /// events without a branch get the workspace's last branch. Returns the number of cwds that
    /// changed project or client and the number of events rewritten. Does nothing under
    /// redaction: the store no longer holds the real cwds, so a rule change applies to cwds first
    /// seen afterwards.
    pub fn reresolve(
        &mut self,
        store: &mut Store,
        resolver: &ProjectResolver,
        scope: Reresolve,
        agents: &[AgentId],
        now: Timestamp,
    ) -> Result<(u64, u64), IngestError> {
        if self.redaction.is_some() {
            return Ok((0, 0));
        }
        let candidates: Vec<Utf8PathBuf> = self
            .map
            .iter()
            .filter(|(cwd, stored)| match scope {
                Reresolve::All => true,
                Reresolve::StaleAndFallbacks => {
                    stored.source == ProjectSource::Cwd
                        || lacks_remotes(stored)
                        || is_stale(resolver, cwd, stored)
                }
                Reresolve::Stale => is_stale(resolver, cwd, stored),
            })
            .map(|(cwd, _)| cwd.clone())
            .collect();
        let mut changed: Vec<ProjectResolution> = Vec::new();
        let mut branches: Vec<(Utf8PathBuf, String)> = Vec::new();
        let mut moved = 0_u64;
        for cwd in candidates {
            let Some(mut found) = resolver.resolve(&cwd, now, &mut self.hints) else {
                continue;
            };
            if found.resolution.source == ProjectSource::Inferred
                && let Some(branch) = self.hints.for_cwd(&cwd).and_then(|h| h.branch.clone())
            {
                branches.push((cwd.clone(), branch));
            }
            let Some(old) = self.map.get(&cwd) else {
                continue;
            };
            // A git-backed answer is not degraded to the cwd fallback because the cwd cannot
            // be read now (a deleted worktree): re-key it from its stored remotes, or keep it.
            if found.problem.is_some()
                && matches!(
                    old.source,
                    ProjectSource::Remote | ProjectSource::RepoRoot | ProjectSource::Inferred
                )
            {
                let Some(rekeyed) = resolver.rekey(old) else {
                    continue;
                };
                found.resolution = rekeyed;
            }
            if old.source == ProjectSource::Cwd && found.problem.is_none() {
                for agent in agents {
                    for kind in [IssueKind::MissingCwd, IssueKind::ProtectedFolder] {
                        store.close_parse_issue(*agent, kind, cwd.as_str(), now)?;
                    }
                }
            }
            if same_answer(old, &found.resolution) {
                continue;
            }
            if old.project != found.resolution.project || old.client != found.resolution.client {
                moved += 1;
            }
            changed.push(found.resolution.clone());
            self.map.insert(cwd, found.resolution);
        }
        // One transaction saves the answers and repoints the events of the cwds whose project or
        // client moved; events that already agree keep their revision.
        let events = store.reassign_projects(&changed)?;
        store.fill_branches(&branches)?;
        for agent in agents {
            if let Some(issue) = self.drift_issue(*agent, now) {
                store.record_parse_issue(&issue)?;
            }
        }
        Ok((moved, events))
    }
}

/// Whether the stored resolution of `cwd` disagrees with what the rules say now. Pure: no
/// filesystem access, so checking every cwd on every scan is cheap.
fn is_stale(resolver: &ProjectResolver, cwd: &Utf8Path, stored: &ProjectResolution) -> bool {
    match (resolver.rule_for(cwd), &stored.source) {
        (None, ProjectSource::Rule(_)) => true,
        // `[git] default_remote` changed which remote keys it.
        (None, ProjectSource::Remote | ProjectSource::Inferred) => resolver.rekey(stored).is_some(),
        (None, _) => false,
        (Some(rule), ProjectSource::Rule(id)) => {
            id != rule.pattern()
                || stored.project != rule.name
                || stored.client.as_deref()
                    != rule
                        .client
                        .as_ref()
                        .map(tokenmtr_config::ClientName::as_str)
        }
        (Some(_), _) => true,
    }
}

/// Whether `stored` is keyed by a remote but its repository's remotes were never read (stored
/// before remotes were kept): re-resolved once per process to fill them in.
fn lacks_remotes(stored: &ProjectResolution) -> bool {
    matches!(
        stored.source,
        ProjectSource::Remote | ProjectSource::Inferred
    ) && stored.remote.is_some()
        && stored.repo_root.is_some()
        && stored.remotes.is_empty()
}

/// Whether `b` would store nothing new over `a`. A `b` without remotes keeps `a`'s stored ones.
fn same_answer(a: &ProjectResolution, b: &ProjectResolution) -> bool {
    a.project == b.project
        && a.name == b.name
        && a.client == b.client
        && a.source == b.source
        && (b.remotes.is_empty() || a.remotes == b.remotes)
}

fn cwd_issue(agent: AgentId, cwd: &Utf8Path, problem: CwdProblem, at: Timestamp) -> ParseIssue {
    let (kind, sample) = match problem {
        CwdProblem::Missing => (
            IssueKind::MissingCwd,
            "directory not found; add a [[projects]] rule to name its project",
        ),
        CwdProblem::Protected => (
            IssueKind::ProtectedFolder,
            "protected folder (not read, to avoid a macOS prompt) or access denied; add a [[projects]] rule",
        ),
        // The store has no finer kind: like a missing cwd, only a rule can name its project.
        CwdProblem::Unreadable => (
            IssueKind::MissingCwd,
            "not a readable directory; add a [[projects]] rule to name its project",
        ),
    };
    ParseIssue {
        agent,
        kind,
        subject: cwd.as_str().to_owned(),
        count: std::num::NonZeroU64::MIN,
        sample: Some(sample.to_owned()),
        at,
    }
}
