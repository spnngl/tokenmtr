//! The `project_map` table: working directory to project resolution (D31).

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
    /// The working directory itself: no rule, no git. The project is "unresolved".
    Cwd,
}

impl ProjectSource {
    fn parts(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Rule(id) => ("rule", Some(id)),
            Self::Remote => ("remote", None),
            Self::RepoRoot => ("repo_root", None),
            Self::Cwd => ("cwd", None),
        }
    }

    fn from_parts(source: &str, rule: Option<String>) -> Result<Self, CodecError> {
        match (source, rule) {
            ("rule", Some(id)) => Ok(Self::Rule(id)),
            ("remote", None) => Ok(Self::Remote),
            ("repo_root", None) => Ok(Self::RepoRoot),
            ("cwd", None) => Ok(Self::Cwd),
            (other, _) => Err(CodecError::Value {
                column: "source",
                value: other.to_owned(),
            }),
        }
    }
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
    /// Normalized git remote, when there is one.
    pub remote: Option<String>,
    /// When the resolution was computed.
    pub resolved_at: Timestamp,
    /// How it was resolved.
    pub source: ProjectSource,
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
    Ok(())
}

/// Saves `resolutions` and points the events of each directory at its project and client.
///
/// Only events whose `(project, client)` differ are rewritten; each gets its own revision (D17)
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

/// Distinct working directories of stored events that have no resolution yet, ordered.
pub(crate) fn unresolved(conn: &Connection) -> Result<Vec<Utf8PathBuf>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT DISTINCT cwd FROM usage_event \
         WHERE cwd IS NOT NULL AND cwd NOT IN (SELECT cwd FROM project_map) ORDER BY cwd",
    )?;
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
    statement
        .query_and_then([cwd.as_str()], decode)?
        .next()
        .transpose()
}

/// Every resolution, ordered by directory.
pub(crate) fn all(conn: &Connection) -> Result<Vec<ProjectResolution>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM project_map ORDER BY cwd");
    let mut statement = conn.prepare_cached(&sql)?;
    statement.query_and_then([], decode)?.collect()
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
    })
}
