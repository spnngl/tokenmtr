//! Project resolution (D31): working directory → project key, display name and client.
//!
//! Order, first hit wins:
//! 1. The first `[[projects]]` rule whose glob matches the cwd. No filesystem access.
//! 2. The git repository containing the cwd (`gix`, never a spawned `git`): a linked worktree
//!    resolves to its main repository. The normalized `remote.origin.url` is the key, so clones
//!    of one remote on several machines or paths are one project.
//! 3. The main repository's root path, when it has no `origin`.
//! 4. The cwd itself.
//!
//! A cwd under a macOS protected folder (D50) is never touched: the check is lexical, so the
//! daemon cannot trigger a consent prompt. A cwd that no longer exists, that the OS refuses to
//! list or that is not a readable directory also skips the git steps. Each is reported as a
//! [`CwdProblem`] so the caller can raise one issue.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_config::ProjectRule;
use tokenmtr_core::ProjectKey;
use tokenmtr_store::{ProjectResolution, ProjectSource};

/// Why a cwd could not be inspected for git metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CwdProblem {
    /// The directory does not exist (deleted, or on another machine). Suggest a `[[projects]]` rule.
    Missing,
    /// Under a protected folder, or the OS denied access (macOS, D50).
    Protected,
    /// It exists but cannot be read as a directory (not a directory, a symlink loop, ...).
    Unreadable,
}

/// A resolution, with the problem that forced the cwd fallback, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The resolution to store.
    pub resolution: ProjectResolution,
    /// Set when the git steps were skipped.
    pub problem: Option<CwdProblem>,
}

/// Resolves working directories against the configured rules.
#[derive(Debug, Clone, Default)]
pub struct ProjectResolver {
    rules: Vec<ProjectRule>,
    /// Folders whose cwds are never read ([`tokenmtr_platform::Paths::protected`]).
    protected: Vec<Utf8PathBuf>,
}

impl ProjectResolver {
    /// A resolver over `rules`, in config order, that never reads under `protected`.
    #[must_use]
    pub const fn new(rules: Vec<ProjectRule>, protected: Vec<Utf8PathBuf>) -> Self {
        Self { rules, protected }
    }

    /// The first rule matching `cwd`.
    #[must_use]
    pub fn rule_for(&self, cwd: &Utf8Path) -> Option<&ProjectRule> {
        self.rules.iter().find(|rule| rule.matches(cwd))
    }

    /// Resolves `cwd`; the cwd itself is the last fallback. `None` only for a blank cwd, which
    /// names no project.
    #[must_use]
    pub fn resolve(&self, cwd: &Utf8Path, at: Timestamp) -> Option<Resolved> {
        if cwd.as_str().trim().is_empty() {
            return None;
        }
        if let Some(rule) = self.rule_for(cwd) {
            return Some(Resolved {
                resolution: rule_resolution(rule, cwd, at),
                problem: None,
            });
        }
        let protected = self
            .protected
            .iter()
            .any(|folder| tokenmtr_platform::paths::is_under(cwd, folder));
        let problem = if protected {
            Some(CwdProblem::Protected)
        } else {
            match std::fs::read_dir(cwd) {
                Ok(_) => None,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Some(CwdProblem::Missing),
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                    Some(CwdProblem::Protected)
                }
                Err(_) => Some(CwdProblem::Unreadable),
            }
        };
        let git = if problem.is_none() {
            git_facts(cwd)
        } else {
            None
        };
        let resolution = match git.and_then(|facts| git_resolution(facts, cwd, at)) {
            Some(resolution) => resolution,
            None => cwd_resolution(cwd, at)?,
        };
        Some(Resolved {
            resolution,
            problem,
        })
    }
}

fn rule_resolution(rule: &ProjectRule, cwd: &Utf8Path, at: Timestamp) -> ProjectResolution {
    ProjectResolution {
        cwd: cwd.to_owned(),
        project: rule.name.clone(),
        name: rule.name.as_str().to_owned(),
        client: rule.client.as_ref().map(|c| c.as_str().to_owned()),
        repo_root: None,
        remote: None,
        resolved_at: at,
        source: ProjectSource::Rule(rule.pattern().to_owned()),
    }
}

fn cwd_resolution(cwd: &Utf8Path, at: Timestamp) -> Option<ProjectResolution> {
    Some(ProjectResolution {
        cwd: cwd.to_owned(),
        project: ProjectKey::try_from(cwd.as_str()).ok()?,
        name: cwd.file_name().unwrap_or(cwd.as_str()).to_owned(),
        client: None,
        repo_root: None,
        remote: None,
        resolved_at: at,
        source: ProjectSource::Cwd,
    })
}

fn git_resolution(facts: GitFacts, cwd: &Utf8Path, at: Timestamp) -> Option<ProjectResolution> {
    let (project, name, source) = match &facts.remote {
        Some(remote) => (
            ProjectKey::try_from(remote.as_str()).ok()?,
            remote.rsplit('/').next().unwrap_or(remote).to_owned(),
            ProjectSource::Remote,
        ),
        None => (
            ProjectKey::try_from(facts.root.as_str()).ok()?,
            facts
                .root
                .file_name()
                .unwrap_or(facts.root.as_str())
                .to_owned(),
            ProjectSource::RepoRoot,
        ),
    };
    Some(ProjectResolution {
        cwd: cwd.to_owned(),
        project,
        name,
        client: None,
        repo_root: Some(facts.root),
        remote: facts.remote,
        resolved_at: at,
        source,
    })
}

struct GitFacts {
    root: Utf8PathBuf,
    remote: Option<String>,
}

fn git_facts(cwd: &Utf8Path) -> Option<GitFacts> {
    let repo = gix::discover(cwd.as_std_path()).ok()?;
    let main = repo.main_repo().unwrap_or_else(|_| repo.clone());
    let root_path = main
        .workdir()
        .and_then(Utf8Path::from_path)
        .or_else(|| Utf8Path::from_path(main.common_dir()).and_then(Utf8Path::parent))?;
    let root = root_path
        .as_std_path()
        .canonicalize()
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| root_path.to_owned());
    let remote = main
        .config_snapshot()
        .string("remote.origin.url")
        .and_then(|url| normalize_remote(&url.to_string()));
    Some(GitFacts { root, remote })
}

/// Normalizes a git remote URL to `host/owner/repo`: scheme, user, port, a trailing `/` and a
/// trailing `.git` are dropped and the host is lowercased. `https://github.com/o/r.git` and
/// `git@github.com:o/r.git` are the same key. Local paths and `file://` URLs give `None`: they
/// identify no shared remote.
#[must_use]
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let (authority_and_path, scp) = match url.split_once("://") {
        Some((scheme, rest)) if !scheme.eq_ignore_ascii_case("file") => (rest.to_owned(), false),
        Some(_) => return None,
        None => {
            // scp-like `[user@]host:path`; the colon must come before any slash.
            let (host, path) = url.split_once(':')?;
            if host.contains('/') || host.is_empty() {
                return None;
            }
            (format!("{host}/{}", path.trim_start_matches('/')), true)
        }
    };
    let (authority, path) = authority_and_path.split_once('/')?;
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = if scp {
        host
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{path}", host.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests;
