//! The `project_map` cache and re-resolution after a rule change (D31).
//!
//! A cwd is resolved once and stored. Changing a `[[projects]]` rule re-resolves the cwds whose
//! stored answer no longer holds and rewrites the project and client of their events; the store
//! bumps a revision only on rows that really change, so sinks re-export just those.

use std::borrow::Cow;
use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_core::{AgentId, ProjectKey, UsageEvent};
use tokenmtr_store::{
    IssueKind, ParseIssue, ProjectResolution, ProjectSource, RedactionKey, Store, StoreRead as _,
};

use super::error::IngestError;
use crate::project::{CwdProblem, ProjectResolver};

/// A cwd's project and client, as stored on events.
pub type Assignment = (ProjectKey, Option<String>);

/// Resolutions loaded from the store, extended as new cwds appear.
pub struct ProjectCache {
    map: HashMap<Utf8PathBuf, ProjectResolution>,
    /// Set under `privacy.redact_paths`: the key that replaces every stored path (D13).
    redaction: Option<RedactionKey>,
}

impl ProjectCache {
    /// Loads every stored resolution. With `redaction`, the cache is keyed by hashed cwds and
    /// nothing path-like reaches the store.
    pub fn load(store: &Store, redaction: Option<RedactionKey>) -> Result<Self, IngestError> {
        let map = store
            .projects()?
            .into_iter()
            .map(|resolution| (resolution.cwd.clone(), resolution))
            .collect();
        Ok(Self { map, redaction })
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
    /// cannot be inspected raises one issue (`missing_cwd` or `protected_folder`), only the
    /// first time it is seen. `None` for a blank cwd.
    pub fn assign(
        &mut self,
        store: &mut Store,
        resolver: &ProjectResolver,
        agent: AgentId,
        cwd: &Utf8Path,
        now: Timestamp,
    ) -> Result<Option<Assignment>, IngestError> {
        let stored_cwd = self.stored_cwd(cwd);
        if let Some(known) = self.map.get(&*stored_cwd) {
            return Ok(Some((known.project.clone(), known.client.clone())));
        }
        // Resolve from the real cwd, then hide it: rules and git need the real path.
        let Some(found) = resolver.resolve(cwd, now) else {
            return Ok(None);
        };
        let Some(resolution) = self.redacted(found.resolution) else {
            return Ok(None);
        };
        store.save_project(&resolution)?;
        if let Some(problem) = found.problem {
            store.record_parse_issue(&cwd_issue(agent, &stored_cwd, problem, now))?;
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
        resolution.repo_root = None;
        if matches!(
            resolution.source,
            ProjectSource::RepoRoot | ProjectSource::Cwd
        ) {
            resolution.project =
                ProjectKey::try_from(key.hash(resolution.project.as_str())).ok()?;
        }
        Some(resolution)
    }

    /// Re-resolves the cwds whose stored resolution a rule change invalidated (all of them when
    /// `all`), saves the new answers and rewrites the events of cwds whose project or client
    /// moved. Returns the number of cwds that changed project or client and the number of events
    /// rewritten. Does nothing under redaction: the store no longer holds the real cwds, so a
    /// rule change applies to cwds first seen afterwards.
    pub fn reresolve(
        &mut self,
        store: &mut Store,
        resolver: &ProjectResolver,
        all: bool,
        now: Timestamp,
    ) -> Result<(u64, u64), IngestError> {
        if self.redaction.is_some() {
            return Ok((0, 0));
        }
        let candidates: Vec<Utf8PathBuf> = self
            .map
            .iter()
            .filter(|(cwd, stored)| all || is_stale(resolver, cwd, stored))
            .map(|(cwd, _)| cwd.clone())
            .collect();
        let mut changed: Vec<ProjectResolution> = Vec::new();
        let mut moved = 0_u64;
        for cwd in candidates {
            let Some(found) = resolver.resolve(&cwd, now) else {
                continue;
            };
            let Some(old) = self.map.get(&cwd) else {
                continue;
            };
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
        Ok((moved, events))
    }
}

/// Whether the stored resolution of `cwd` disagrees with what the rules say now. Pure: no
/// filesystem access, so checking every cwd on every scan is cheap.
fn is_stale(resolver: &ProjectResolver, cwd: &Utf8Path, stored: &ProjectResolution) -> bool {
    match (resolver.rule_for(cwd), &stored.source) {
        (None, ProjectSource::Rule(_)) => true,
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

fn same_answer(a: &ProjectResolution, b: &ProjectResolution) -> bool {
    a.project == b.project && a.name == b.name && a.client == b.client && a.source == b.source
}

fn cwd_issue(agent: AgentId, cwd: &Utf8Path, problem: CwdProblem, at: Timestamp) -> ParseIssue {
    let (kind, sample) = match problem {
        CwdProblem::Missing => (
            IssueKind::MissingCwd,
            "directory not found; add a [[projects]] rule to name its project",
        ),
        CwdProblem::Protected => (
            IssueKind::ProtectedFolder,
            "access denied (protected folder); grant access or add a [[projects]] rule",
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
