//! Superset's host database, read-only, as a hint source for cwds git cannot inspect.
//!
//! Superset (a worktree manager for coding agents) keeps one SQLite database per host at
//! `<dir>/host/<uuid>/host.db`. Its `workspaces` rows map a worktree path to a branch and a
//! project; `projects` rows give the main repository path and remote URL. A worktree Superset
//! deleted leaves no git metadata behind, but its row may remain (archived), so the row still
//! names the repository and the workspace's last branch.
//!
//! The database belongs to another app: it is opened read-only, never written, and its schema
//! may change. A missing table or column turns the hints off and is reported as
//! [`Hints::drift`]; it never fails a scan.

use camino::{Utf8Path, Utf8PathBuf};
use rusqlite::{Connection, OpenFlags};

/// What Superset knows about one worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeHint {
    /// The main repository's path (`projects.repo_path`).
    pub repo_path: Option<Utf8PathBuf>,
    /// The repository's remote URL (`projects.repo_url`), not normalized.
    pub repo_url: Option<String>,
    /// The workspace's last branch: the branch Superset recorded, which a live worktree may
    /// since have switched away from.
    pub branch: Option<String>,
}

/// The hints of one scan, read on first use.
#[derive(Debug)]
pub struct Hints {
    state: State,
}

#[derive(Debug)]
enum State {
    Off,
    Unread(Utf8PathBuf),
    /// Worktree paths, longest first, so the deepest worktree containing a cwd wins.
    Read(Vec<(Utf8PathBuf, WorktreeHint)>),
    Drifted(String),
}

impl Hints {
    /// No hints (the integration is off).
    #[must_use]
    pub const fn off() -> Self {
        Self { state: State::Off }
    }

    /// Hints read from the Superset data directory `dir` on first use.
    #[must_use]
    pub const fn superset(dir: Utf8PathBuf) -> Self {
        Self {
            state: State::Unread(dir),
        }
    }

    /// Hints from `worktrees` (tests).
    #[cfg(test)]
    #[must_use]
    pub fn from_worktrees(mut worktrees: Vec<(Utf8PathBuf, WorktreeHint)>) -> Self {
        sort(&mut worktrees);
        Self {
            state: State::Read(worktrees),
        }
    }

    /// The worktree containing `cwd`, the deepest when they nest.
    pub fn for_cwd(&mut self, cwd: &Utf8Path) -> Option<&WorktreeHint> {
        if let State::Unread(dir) = &self.state {
            self.state = match read(dir) {
                Ok(worktrees) => State::Read(worktrees),
                Err(Failure::Drift(message)) => State::Drifted(message),
                Err(Failure::Unavailable(message)) => {
                    tracing::warn!(error = %message, "superset database unavailable");
                    State::Off
                }
            };
        }
        match &self.state {
            State::Read(worktrees) => worktrees
                .iter()
                .find(|(path, _)| cwd.starts_with(path))
                .map(|(_, hint)| hint),
            State::Off | State::Unread(_) | State::Drifted(_) => None,
        }
    }

    /// Why the hints are off, when Superset's schema is not the one this version knows.
    #[must_use]
    pub fn drift(&self) -> Option<&str> {
        match &self.state {
            State::Drifted(message) => Some(message),
            State::Off | State::Unread(_) | State::Read(_) => None,
        }
    }

    /// Whether the database was read with the known schema.
    #[must_use]
    pub const fn read_ok(&self) -> bool {
        matches!(self.state, State::Read(_))
    }
}

enum Failure {
    /// The schema changed: a table, column or type this reader expects is gone.
    Drift(String),
    /// Anything else (locked, unreadable): retried next scan, not an issue.
    Unavailable(String),
}

impl From<rusqlite::Error> for Failure {
    fn from(err: rusqlite::Error) -> Self {
        let message = err.to_string();
        // A missing table or column is a prepare error, whose variant depends on SQLite's
        // build options; its text does not.
        let drift = matches!(
            err,
            rusqlite::Error::InvalidColumnType(..) | rusqlite::Error::InvalidColumnName(_)
        ) || message.contains("no such table")
            || message.contains("no such column");
        if drift {
            Self::Drift(message)
        } else {
            Self::Unavailable(message)
        }
    }
}

const WORKTREES: &str = "SELECT w.worktree_path, w.branch, p.repo_path, p.repo_url \
     FROM workspaces w LEFT JOIN projects p ON p.id = w.project_id";

/// Every worktree of every host database under `dir`. A missing directory has none.
fn read(dir: &Utf8Path) -> Result<Vec<(Utf8PathBuf, WorktreeHint)>, Failure> {
    let hosts = match std::fs::read_dir(dir.join("host")) {
        Ok(hosts) => hosts,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Failure::Unavailable(err.to_string())),
    };
    let mut worktrees = Vec::new();
    for host in hosts.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(host.path().join("host.db")) else {
            continue;
        };
        if path.is_file() {
            read_host(&path, &mut worktrees)?;
        }
    }
    sort(&mut worktrees);
    Ok(worktrees)
}

fn read_host(
    path: &Utf8Path,
    worktrees: &mut Vec<(Utf8PathBuf, WorktreeHint)>,
) -> Result<(), Failure> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(1))?;
    let mut rows = conn.prepare(WORKTREES)?;
    let found = rows.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            WorktreeHint {
                branch: non_blank(row.get(1)?),
                repo_path: non_blank(row.get(2)?).map(Utf8PathBuf::from),
                repo_url: non_blank(row.get(3)?),
            },
        ))
    })?;
    for row in found {
        let (worktree, hint) = row?;
        if let Some(worktree) = non_blank(worktree).map(Utf8PathBuf::from)
            && worktree.is_absolute()
        {
            worktrees.push((worktree, hint));
        }
    }
    Ok(())
}

fn non_blank(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.trim().is_empty())
}

fn sort(worktrees: &mut [(Utf8PathBuf, WorktreeHint)]) {
    worktrees.sort_by_key(|(path, _)| std::cmp::Reverse(path.as_str().len()));
}

#[cfg(test)]
pub(crate) mod fixture {
    use camino::Utf8Path;
    use rusqlite::Connection;

    /// A host database under the Superset directory `dir`, shaped like Superset's, with one
    /// workspace: `worktree` on branch `feat/otter` of the repository at `repo_path`, whose
    /// remote is `github.com/acme/widgets`.
    pub(crate) fn fake_superset(dir: &Utf8Path, worktree: &Utf8Path, repo_path: &Utf8Path) {
        let host = dir.join("host").join("406aa624");
        std::fs::create_dir_all(&host).unwrap();
        let conn = Connection::open(host.join("host.db")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO projects (id, repo_path, repo_url) VALUES ('p1', ?1, 'https://github.com/acme/widgets')",
            [repo_path.as_str()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workspaces (id, project_id, worktree_path, branch) \
             VALUES ('w1', 'p1', ?1, 'feat/otter')",
            [worktree.as_str()],
        )
        .unwrap();
    }

    /// The tables and columns read here, as Superset declares them.
    const SCHEMA: &str = "CREATE TABLE projects (id TEXT PRIMARY KEY, repo_path TEXT NOT NULL, repo_url TEXT, deleted_at INTEGER);
         CREATE TABLE workspaces (id TEXT PRIMARY KEY, project_id TEXT, worktree_path TEXT NOT NULL, branch TEXT NOT NULL, archived_at INTEGER);";
}

#[cfg(test)]
mod tests {
    use super::fixture::fake_superset;
    use super::*;

    #[test]
    fn reads_the_worktree_of_a_cwd_inside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        fake_superset(
            dir,
            Utf8Path::new("/gone/worktrees/widgets/brave-otter"),
            Utf8Path::new("/gone/src/widgets"),
        );
        let mut hints = Hints::superset(dir.to_owned());
        let hint = hints
            .for_cwd(Utf8Path::new("/gone/worktrees/widgets/brave-otter/crates"))
            .cloned();
        assert_eq!(
            hint,
            Some(WorktreeHint {
                repo_path: Some(Utf8PathBuf::from("/gone/src/widgets")),
                repo_url: Some("https://github.com/acme/widgets".to_owned()),
                branch: Some("feat/otter".to_owned()),
            })
        );
        assert!(hints.read_ok());
        assert_eq!(hints.for_cwd(Utf8Path::new("/gone/worktrees/other")), None);
    }

    #[test]
    fn a_missing_column_is_drift_not_a_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let host = dir.join("host").join("h");
        std::fs::create_dir_all(&host).unwrap();
        Connection::open(host.join("host.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE projects (id TEXT, repo_path TEXT);
                 CREATE TABLE workspaces (id TEXT, project_id TEXT, worktree_path TEXT);",
            )
            .unwrap();
        let mut hints = Hints::superset(dir.to_owned());
        assert_eq!(hints.for_cwd(Utf8Path::new("/x")), None);
        assert!(hints.drift().is_some_and(|m| m.contains("no such column")));
    }

    #[test]
    fn no_superset_directory_means_no_hints() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().join("absent");
        let mut hints = Hints::superset(dir);
        assert_eq!(hints.for_cwd(Utf8Path::new("/x")), None);
        assert!(hints.read_ok());
        assert_eq!(hints.drift(), None);
    }
}
