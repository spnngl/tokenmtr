//! Embedded, versioned schema migrations.
//!
//! The schema version is `PRAGMA user_version`, which `rusqlite_migration` sets to the number of
//! applied migrations. Appending a migration means appending to [`migrations`] and raising
//! [`SCHEMA_VERSION`]; a test keeps the two in step.

use rusqlite::Transaction;
use rusqlite_migration::{HookResult, M, Migrations};
use uuid::Uuid;

/// The schema version this build reads and writes: the number of migrations in `migrations()`.
pub const SCHEMA_VERSION: u32 = 7;

/// All migrations, oldest first.
pub(crate) fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up_with_hook(include_str!("../migrations/0001_init.sql"), seed_meta),
        M::up(include_str!("../migrations/0002_usage_event_cwd.sql")),
        M::up(include_str!(
            "../migrations/0003_superseded_removed_quota_index.sql"
        )),
        M::up(include_str!(
            "../migrations/0004_ingest_run_agent_index.sql"
        )),
        M::up(include_str!("../migrations/0005_quota_snapshot_money.sql")),
        M::up(include_str!(
            "../migrations/0006_inferred_project_keychain_denied.sql"
        )),
        M::up(include_str!("../migrations/0007_project_remote.sql")),
    ])
}

/// Seeds the identity rows in the same transaction as the schema.
fn seed_meta(tx: &Transaction<'_>) -> HookResult {
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('machine_id', ?1), ('redaction_key', randomblob(32))",
        [Uuid::now_v7().to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_valid() {
        migrations().validate().unwrap();
    }

    #[test]
    fn schema_version_counts_the_migrations() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn applying_twice_changes_nothing() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let machine: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'machine_id'", [], |r| {
                r.get(0)
            })
            .unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let again: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'machine_id'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(machine, again);
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(tables >= 17);
    }

    #[test]
    fn project_remote_renames_remote_keyed_projects_to_owner_and_repo() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 6).unwrap();
        conn.execute_batch(
            "INSERT INTO project_map (cwd, project, name, client, repo_root, remote, resolved_at_us, source, rule_id) VALUES \
             ('/a', 'github.com/me/images', 'images', NULL, '/a', 'github.com/me/images', 0, 'remote', NULL), \
             ('/b', 'gitlab.com/g/sub/repo', 'repo', NULL, NULL, 'gitlab.com/g/sub/repo', 0, 'inferred', NULL), \
             ('/c', 'mine', 'Mine', NULL, '/c', 'github.com/me/c', 0, 'rule', 'r'), \
             ('/d', '/d', 'd', NULL, '/d', NULL, 0, 'repo_root', NULL);",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let names: Vec<String> = conn
            .prepare("SELECT name FROM project_map ORDER BY cwd")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(names, ["me/images", "g/sub/repo", "Mine", "d"]);
        let remotes: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_remote", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remotes, 0);
    }
}
