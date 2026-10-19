//! The `project_map` table: working directory to project resolution; and `project_remote`, the
//! git remotes of each repository root.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use rusqlite::{Connection, Row, named_params, params};
use tokenmtr_core::ProjectKey;

use crate::codec::{RowExt, to_us};
use crate::error::{CodecError, StoreError};
use crate::types::Dataset;
use crate::versioned;

/// How a project was resolved, best evidence first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSource {
    /// A `[[projects]]` rule of the config, by its id.
    Rule(String),
    /// The git remote of the repository.
    Remote,
    /// The git repository root, for repositories without a remote.
    RepoRoot,
    /// Git could not inspect the cwd (deleted worktree, folder outside git): another app's record
    /// of it (Superset's workspaces) named its repository. The key is the remote when known,
    /// else the repository root, like [`Self::Remote`] and [`Self::RepoRoot`].
    Inferred,
    /// The working directory itself: no rule, no git. The project is "unresolved".
    Cwd,
}

impl ProjectSource {
    fn parts(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Rule(id) => ("rule", Some(id)),
            Self::Remote => ("remote", None),
            Self::RepoRoot => ("repo_root", None),
            Self::Inferred => ("inferred", None),
            Self::Cwd => ("cwd", None),
        }
    }

    fn from_parts(source: &str, rule: Option<String>) -> Result<Self, CodecError> {
        match (source, rule) {
            ("rule", Some(id)) => Ok(Self::Rule(id)),
            ("remote", None) => Ok(Self::Remote),
            ("repo_root", None) => Ok(Self::RepoRoot),
            ("inferred", None) => Ok(Self::Inferred),
            ("cwd", None) => Ok(Self::Cwd),
            (other, _) => Err(CodecError::Value {
                column: "source",
                value: other.to_owned(),
            }),
        }
    }
}

/// One git remote of a repository, without credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRemote {
    /// Remote name, such as `origin` or `upstream`.
    pub name: String,
    /// Normalized URL, `host/owner/repo`.
    pub url: String,
    /// The remote's default branch (`refs/remotes/<name>/HEAD`), when the clone recorded it.
    pub default_branch: Option<String>,
}

/// The project a working directory resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectResolution {
    /// The directory; the key.
    pub cwd: Utf8PathBuf,
    /// The project key stored on events.
    pub project: ProjectKey,
    /// Display name.
    pub name: String,
    /// Client label from a rule.
    pub client: Option<String>,
    /// Git repository root, when inside one.
    pub repo_root: Option<Utf8PathBuf>,
    /// Normalized git remote that keys the project, when there is one.
    pub remote: Option<String>,
    /// When the resolution was computed.
    pub resolved_at: Timestamp,
    /// How it was resolved.
    pub source: ProjectSource,
    /// Every remote of `repo_root` with a shared URL, by name; empty without a repository root.
    /// Saving a resolution with remotes replaces the stored remotes of its root; saving one
    /// without (a rule, or a repository known only from a hint) keeps them.
    pub remotes: Vec<GitRemote>,
}

const COLUMNS: &str =
    "cwd, project, name, client, repo_root, remote, resolved_at_us, source, rule_id";

const SAVE: &str = "INSERT INTO project_map \
    (cwd, project, name, client, repo_root, remote, resolved_at_us, source, rule_id) \
    VALUES (:cwd, :project, :name, :client, :repo_root, :remote, :resolved_at_us, :source, :rule_id) \
    ON CONFLICT (cwd) DO UPDATE SET project = excluded.project, name = excluded.name, \
    client = excluded.client, repo_root = excluded.repo_root, remote = excluded.remote, \
    resolved_at_us = excluded.resolved_at_us, source = excluded.source, rule_id = excluded.rule_id";

/// Inserts or replaces the resolution of `resolution.cwd`.
pub(crate) fn save(conn: &Connection, resolution: &ProjectResolution) -> Result<(), StoreError> {
    let (source, rule) = resolution.source.parts();
    conn.prepare_cached(SAVE)?.execute(named_params! {
        ":cwd": resolution.cwd.as_str(),
        ":project": resolution.project.as_str(),
        ":name": resolution.name,
        ":client": resolution.client,
        ":repo_root": resolution.repo_root.as_ref().map(|p| p.as_str()),
        ":remote": resolution.remote,
        ":resolved_at_us": to_us(resolution.resolved_at),
        ":source": source,
        ":rule_id": rule,
    })?;
    if let Some(root) = &resolution.repo_root
        && !resolution.remotes.is_empty()
    {
        conn.prepare_cached("DELETE FROM project_remote WHERE repo_root = ?1")?
            .execute([root.as_str()])?;
        let mut insert = conn.prepare_cached(
            "INSERT INTO project_remote (repo_root, name, url, default_branch) \
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for remote in &resolution.remotes {
            insert.execute(params![
                root.as_str(),
                remote.name,
                remote.url,
                remote.default_branch
            ])?;
        }
    }
    Ok(())
}

/// The stored remotes of `root`, by name.
fn remotes(conn: &Connection, root: &str) -> Result<Vec<GitRemote>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT name, url, default_branch FROM project_remote WHERE repo_root = ?1 ORDER BY name",
    )?;
    let rows = statement
        .query_map([root], |row| {
            Ok(GitRemote {
                name: row.get(0)?,
                url: row.get(1)?,
                default_branch: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The remotes of every repository root that `project` spans, by name then URL, without
/// duplicates.
pub(crate) fn project_remotes(
    conn: &Connection,
    project: &ProjectKey,
) -> Result<Vec<GitRemote>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT DISTINCT r.name, r.url, r.default_branch FROM project_remote r \
         JOIN project_map m ON m.repo_root = r.repo_root \
         WHERE m.project = ?1 ORDER BY r.name, r.url, r.default_branch",
    )?;
    let rows = statement
        .query_map([project.as_str()], |row| {
            Ok(GitRemote {
                name: row.get(0)?,
                url: row.get(1)?,
                default_branch: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Saves `resolutions` and points the events of each directory at its project and client.
///
/// Only events whose `(project, client)` differ are rewritten; each gets its own revision
/// and the dataset counter moves once. Returns how many events changed.
pub(crate) fn reassign(
    conn: &Connection,
    now: Timestamp,
    resolutions: &[ProjectResolution],
) -> Result<u64, StoreError> {
    let mut revision = versioned::head(conn, Dataset::UsageEvents)?;
    let mut changed = 0_u64;
    let mut stale = conn.prepare_cached(
        "SELECT id FROM usage_event \
         WHERE cwd = ?1 AND (project, client) IS NOT (?2, ?3) ORDER BY id",
    )?;
    let mut rewrite = conn.prepare_cached(
        "UPDATE usage_event SET project = ?1, client = ?2, revision = ?3, updated_at_us = ?4 \
         WHERE id = ?5",
    )?;
    for resolution in resolutions {
        save(conn, resolution)?;
        let ids = stale
            .query_map(
                params![
                    resolution.cwd.as_str(),
                    resolution.project.as_str(),
                    resolution.client
                ],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            revision = revision.next();
            rewrite.execute(params![
                resolution.project.as_str(),
                resolution.client,
                revision,
                to_us(now),
                id
            ])?;
            changed += 1;
        }
    }
    if changed > 0 {
        conn.execute(
            "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
            params![revision, Dataset::UsageEvents.as_str()],
        )?;
    }
    Ok(changed)
}

/// Sets the git branch of the events of each `(cwd, branch)` that have none. Each changed
/// event gets its own revision and the dataset counter moves once. Returns how many changed.
pub(crate) fn fill_branches(
    conn: &Connection,
    now: Timestamp,
    branches: &[(Utf8PathBuf, String)],
) -> Result<u64, StoreError> {
    let mut revision = versioned::head(conn, Dataset::UsageEvents)?;
    let mut changed = 0_u64;
    let mut missing = conn.prepare_cached(
        "SELECT id FROM usage_event WHERE cwd = ?1 AND git_branch IS NULL ORDER BY id",
    )?;
    let mut fill = conn.prepare_cached(
        "UPDATE usage_event SET git_branch = ?1, revision = ?2, updated_at_us = ?3 WHERE id = ?4",
    )?;
    for (cwd, branch) in branches {
        let ids = missing
            .query_map([cwd.as_str()], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            revision = revision.next();
            fill.execute(params![branch, revision, to_us(now), id])?;
            changed += 1;
        }
    }
    if changed > 0 {
        conn.execute(
            "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
            params![revision, Dataset::UsageEvents.as_str()],
        )?;
    }
    Ok(changed)
}

/// Distinct unresolved working directories. `SELECT DISTINCT cwd` reads every entry of
/// `usage_event_cwd`; skipping from one directory to the next costs one seek per directory.
pub(crate) const UNRESOLVED: &str = "WITH RECURSIVE c(cwd) AS ( \
       SELECT MIN(cwd) FROM usage_event \
       UNION ALL \
       SELECT (SELECT MIN(cwd) FROM usage_event WHERE cwd > c.cwd) FROM c \
       WHERE c.cwd IS NOT NULL) \
     SELECT cwd FROM c \
     WHERE cwd IS NOT NULL AND cwd NOT IN (SELECT cwd FROM project_map) ORDER BY cwd";

/// Distinct working directories of stored events that have no resolution yet, ordered.
pub(crate) fn unresolved(conn: &Connection) -> Result<Vec<Utf8PathBuf>, StoreError> {
    let mut statement = conn.prepare_cached(UNRESOLVED)?;
    let cwds = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(cwds.into_iter().map(Utf8PathBuf::from).collect())
}

/// The resolution of one directory.
pub(crate) fn get(
    conn: &Connection,
    cwd: &Utf8Path,
) -> Result<Option<ProjectResolution>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM project_map WHERE cwd = ?1");
    let mut statement = conn.prepare_cached(&sql)?;
    let found = statement
        .query_and_then([cwd.as_str()], decode)?
        .next()
        .transpose()?;
    found.map(|r| with_remotes(conn, r)).transpose()
}

/// Every resolution, ordered by directory.
pub(crate) fn all(conn: &Connection) -> Result<Vec<ProjectResolution>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM project_map ORDER BY cwd");
    let mut statement = conn.prepare_cached(&sql)?;
    let found: Vec<ProjectResolution> = statement
        .query_and_then([], decode)?
        .collect::<Result<_, _>>()?;
    found.into_iter().map(|r| with_remotes(conn, r)).collect()
}

fn with_remotes(
    conn: &Connection,
    mut resolution: ProjectResolution,
) -> Result<ProjectResolution, StoreError> {
    if let Some(root) = &resolution.repo_root {
        resolution.remotes = remotes(conn, root.as_str())?;
    }
    Ok(resolution)
}

fn decode(row: &Row<'_>) -> Result<ProjectResolution, StoreError> {
    let cwd: String = row.get("cwd")?;
    let repo_root: Option<String> = row.get("repo_root")?;
    let source: String = row.get("source")?;
    Ok(ProjectResolution {
        cwd: cwd.into(),
        project: row.id::<ProjectKey>("project")?,
        name: row.get("name")?,
        client: row.get("client")?,
        repo_root: repo_root.map(Into::into),
        remote: row.get("remote")?,
        resolved_at: row.ts("resolved_at_us")?,
        source: ProjectSource::from_parts(&source, row.get("rule_id")?)?,
        remotes: Vec::new(),
    })
}
