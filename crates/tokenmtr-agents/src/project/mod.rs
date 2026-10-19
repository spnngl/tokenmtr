//! Project resolution: working directory → project key, display name and client.
//!
//! Order, first hit wins:
//! 1. The first `[[projects]]` rule whose glob matches the cwd. No filesystem access.
//! 2. The git repository containing the cwd (`gix`, never a spawned `git`): a linked worktree
//!    resolves to its main repository. The normalized URL of its key remote is the key, so
//!    clones of one remote on several machines or paths are one project, named `owner/repo`.
//!    The key remote is `[git] default_remote`, else `origin`, else the first by name
//!    ([`ProjectResolver::key_remote`]). Every remote with a shared URL is kept on the resolution.
//! 3. The main repository's root path, when no remote has a shared URL.
//! 4. When git cannot inspect the cwd (a deleted worktree, a folder outside git), another app's
//!    record of it ([`superset::Hints`]): the main repository it names goes through steps 2-3,
//!    so the key matches its live worktrees; when that repository is gone too, its recorded
//!    remote URL is the key. Source [`ProjectSource::Inferred`].
//! 5. The cwd itself.
//!
//! A cwd under a macOS protected folder is never touched: the check is lexical, so the
//! daemon cannot trigger a consent prompt. A cwd that no longer exists, that the OS refuses to
//! list or that is not a readable directory also skips the git steps. Each is reported as a
//! [`CwdProblem`] so the caller can raise one issue, unless step 4 named the project.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_config::{GitName, ProjectRule};
use tokenmtr_core::ProjectKey;
use tokenmtr_store::{GitRemote, ProjectResolution, ProjectSource};

pub mod superset;

use superset::{Hints, WorktreeHint};

/// Why a cwd could not be inspected for git metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CwdProblem {
    /// The directory does not exist (deleted, or on another machine). Suggest a `[[projects]]` rule.
    Missing,
    /// Under a protected folder, or the OS denied access (macOS).
    Protected,
    /// It exists but cannot be read as a directory (not a directory, a symlink loop, ...).
    Unreadable,
}

/// A resolution, with the problem that forced the cwd fallback, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The resolution to store.
    pub resolution: ProjectResolution,
    /// Set when the git steps were skipped and no hint named the project.
    pub problem: Option<CwdProblem>,
}

/// Resolves working directories against the configured rules.
#[derive(Debug, Clone)]
pub struct ProjectResolver {
    rules: Vec<ProjectRule>,
    /// Folders whose cwds are never read ([`tokenmtr_platform::Paths::protected`]).
    protected: Vec<Utf8PathBuf>,
    /// The remote whose URL keys a repository's project (`[git] default_remote`).
    default_remote: GitName,
}

impl Default for ProjectResolver {
    fn default() -> Self {
        Self::new(Vec::new(), Vec::new())
    }
}

impl ProjectResolver {
    /// A resolver over `rules`, in config order, that never reads under `protected` and keys
    /// repositories by their `origin` remote.
    #[must_use]
    pub fn new(rules: Vec<ProjectRule>, protected: Vec<Utf8PathBuf>) -> Self {
        Self {
            rules,
            protected,
            default_remote: GitName::origin(),
        }
    }

    /// The same resolver, keying repositories by `remote` first.
    #[must_use]
    pub fn with_default_remote(mut self, remote: GitName) -> Self {
        self.default_remote = remote;
        self
    }

    /// The remote whose URL keys the project: the default remote, else `origin`, else the
    /// first by name. `None` when `remotes` is empty.
    #[must_use]
    pub fn key_remote<'a>(&self, remotes: &'a [GitRemote]) -> Option<&'a GitRemote> {
        let named = |name: &str| remotes.iter().find(|remote| remote.name == name);
        named(self.default_remote.as_str())
            .or_else(|| named("origin"))
            .or_else(|| remotes.iter().min_by(|a, b| a.name.cmp(&b.name)))
    }

    /// `stored` keyed by its stored remotes under the current default remote, when that moves
    /// its key. Pure: re-keys a git-backed answer without reading the repository.
    #[must_use]
    pub fn rekey(&self, stored: &ProjectResolution) -> Option<ProjectResolution> {
        if !matches!(
            stored.source,
            ProjectSource::Remote | ProjectSource::Inferred
        ) {
            return None;
        }
        let url = &self.key_remote(&stored.remotes)?.url;
        if stored.remote.as_ref() == Some(url) {
            return None;
        }
        Some(ProjectResolution {
            project: ProjectKey::try_from(url.as_str()).ok()?,
            name: remote_name(url).to_owned(),
            remote: Some(url.clone()),
            ..stored.clone()
        })
    }

    /// The first rule matching `cwd`.
    #[must_use]
    pub fn rule_for(&self, cwd: &Utf8Path) -> Option<&ProjectRule> {
        self.rules.iter().find(|rule| rule.matches(cwd))
    }

    fn is_protected(&self, path: &Utf8Path) -> bool {
        self.protected
            .iter()
            .any(|folder| tokenmtr_platform::paths::is_under(path, folder))
    }

    /// Resolves `cwd`, asking `hints` when git cannot; the cwd itself is the last fallback.
    /// `None` only for a blank cwd, which names no project.
    #[must_use]
    pub fn resolve(&self, cwd: &Utf8Path, at: Timestamp, hints: &mut Hints) -> Option<Resolved> {
        if cwd.as_str().trim().is_empty() {
            return None;
        }
        if let Some(rule) = self.rule_for(cwd) {
            return Some(Resolved {
                resolution: rule_resolution(rule, cwd, at),
                problem: None,
            });
        }
        let problem = if self.is_protected(cwd) {
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
        if let Some(resolution) = git.and_then(|facts| self.git_resolution(facts, cwd, at)) {
            return Some(Resolved {
                resolution,
                problem,
            });
        }
        if let Some(resolution) = hints
            .for_cwd(cwd)
            .and_then(|hint| self.inferred_resolution(hint, cwd, at))
        {
            return Some(Resolved {
                resolution,
                problem: None,
            });
        }
        Some(Resolved {
            resolution: cwd_resolution(cwd, at)?,
            problem,
        })
    }

    /// The project `hint` names: its main repository through git when it still exists (so the
    /// key matches the repository's live worktrees), else its recorded path and remote URL.
    fn inferred_resolution(
        &self,
        hint: &WorktreeHint,
        cwd: &Utf8Path,
        at: Timestamp,
    ) -> Option<ProjectResolution> {
        let repo = hint.repo_path.as_deref()?;
        let live = (!self.is_protected(repo))
            .then(|| git_facts(repo))
            .flatten();
        let mut resolution = match live {
            Some(facts) => self.git_resolution(facts, cwd, at)?,
            None => keyed_resolution(
                repo.to_owned(),
                hint.repo_url.as_deref().and_then(normalize_remote),
                Vec::new(),
                cwd,
                at,
            )?,
        };
        resolution.source = ProjectSource::Inferred;
        Some(resolution)
    }

    /// The branch checked out in the git repository containing `cwd`, now. `None` under a
    /// protected folder, outside git, or on a detached HEAD.
    #[must_use]
    pub fn head_branch(&self, cwd: &Utf8Path) -> Option<String> {
        if self.is_protected(cwd) {
            return None;
        }
        let repo = gix::discover(cwd.as_std_path()).ok()?;
        let head = repo.head_name().ok()??;
        Some(head.shorten().to_string())
    }

    fn git_resolution(
        &self,
        facts: GitFacts,
        cwd: &Utf8Path,
        at: Timestamp,
    ) -> Option<ProjectResolution> {
        let remote = self.key_remote(&facts.remotes).map(|r| r.url.clone());
        keyed_resolution(facts.root, remote, facts.remotes, cwd, at)
    }
}

/// The display name of a remote-keyed project: the URL path after the host (`owner/repo`,
/// `group/sub/repo`).
fn remote_name(url: &str) -> &str {
    url.split_once('/').map_or(url, |(_, path)| path)
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
        remotes: Vec::new(),
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
        remotes: Vec::new(),
    })
}

/// The resolution of a repository at `root` keyed by `remote`, else by `root`.
fn keyed_resolution(
    root: Utf8PathBuf,
    remote: Option<String>,
    remotes: Vec<GitRemote>,
    cwd: &Utf8Path,
    at: Timestamp,
) -> Option<ProjectResolution> {
    let (project, name, source) = match &remote {
        Some(remote) => (
            ProjectKey::try_from(remote.as_str()).ok()?,
            remote_name(remote).to_owned(),
            ProjectSource::Remote,
        ),
        None => (
            ProjectKey::try_from(root.as_str()).ok()?,
            root.file_name().unwrap_or(root.as_str()).to_owned(),
            ProjectSource::RepoRoot,
        ),
    };
    Some(ProjectResolution {
        cwd: cwd.to_owned(),
        project,
        name,
        client: None,
        repo_root: Some(root),
        remote,
        resolved_at: at,
        source,
        remotes,
    })
}

struct GitFacts {
    root: Utf8PathBuf,
    /// Remotes with a shared URL, by name.
    remotes: Vec<GitRemote>,
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
    let config = main.config_snapshot();
    let remotes = main
        .remote_names()
        .into_iter()
        .filter_map(|name| {
            let name = name.to_string();
            let url = config
                .string(format!("remote.{name}.url").as_str())
                .and_then(|url| normalize_remote(&url.to_string()))?;
            let default_branch = remote_head(&main, &name);
            Some(GitRemote {
                name,
                url,
                default_branch,
            })
        })
        .collect();
    Some(GitFacts { root, remotes })
}

/// The branch `refs/remotes/<remote>/HEAD` points at, short (`main`): the remote's default
/// branch as recorded by the last clone or `git remote set-head`.
fn remote_head(repo: &gix::Repository, remote: &str) -> Option<String> {
    let head = repo
        .try_find_reference(format!("refs/remotes/{remote}/HEAD").as_str())
        .ok()??;
    let target = head.target().try_name()?.as_bstr().to_string();
    let branch = target.strip_prefix(&format!("refs/remotes/{remote}/"))?;
    (!branch.is_empty()).then(|| branch.to_owned())
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
