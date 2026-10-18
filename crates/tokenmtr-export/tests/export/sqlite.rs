//! `SQLite` sink: upsert-by-revision, `_schema`, generated DDL and refused paths.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use camino::Utf8Path;
use rusqlite::Connection;
use tempfile::TempDir;
use tokenmtr_config::{RedactColumn, RedactMode};
use tokenmtr_core::SinkId;
use tokenmtr_export::{
    EXPORT_SCHEMA_VERSION, ExportBatch, ExportRow, Redactor, Sink, SinkError, SqliteSink,
    SqliteTarget, project, schema,
};
use tokenmtr_store::{Dataset, RedactionKey, Revision, Versioned};

use crate::fixtures::{TestResult, live, machine, path_in, quota_sample, usage};

fn sink(path: &Utf8Path) -> TestResult<SqliteSink> {
    Ok(SqliteSink::new(
        SinkId::try_from("db")?,
        SqliteTarget {
            path: path.to_owned(),
        },
    ))
}

fn usage_row(session: &str, revision: u64, output: u64) -> TestResult<ExportRow> {
    let machine = machine()?;
    let mut event = usage(
        &machine,
        session,
        "2026-05-01T10:00:00Z",
        Some("/work/acme"),
    )?;
    event.tokens.output = output;
    Ok(project::usage_event(
        &machine,
        &Versioned {
            revision: Revision::new(revision),
            row: live(event),
        },
    ))
}

fn batch(rows: Vec<ExportRow>) -> ExportBatch {
    ExportBatch {
        dataset: Dataset::UsageEvents,
        rows,
    }
}

fn outputs(path: &Utf8Path) -> TestResult<Vec<(i64, i64)>> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare("SELECT revision, output FROM usage_events ORDER BY event_id")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[test]
fn upsert_replaces_only_when_the_revision_is_higher() -> TestResult {
    let tmp = TempDir::new()?;
    let path = path_in(&tmp, "out/export.db")?;
    let mut sink = sink(&path)?;
    sink.write(&batch(vec![usage_row("a", 2, 50)?]))?;
    assert_eq!(outputs(&path)?, vec![(2, 50)]);

    sink.write(&batch(vec![usage_row("a", 1, 5)?]))?;
    assert_eq!(outputs(&path)?, vec![(2, 50)], "older revision ignored");

    sink.write(&batch(vec![usage_row("a", 2, 50)?]))?;
    assert_eq!(outputs(&path)?, vec![(2, 50)], "same revision is a no-op");

    sink.write(&batch(vec![usage_row("a", 3, 70)?, usage_row("b", 4, 1)?]))?;
    let rows = outputs(&path)?;
    assert_eq!(rows.len(), 2, "one row per id");
    assert!(rows.contains(&(3, 70)) && rows.contains(&(4, 1)));
    Ok(())
}

#[test]
fn schema_table_and_ddl_follow_the_export_schema() -> TestResult {
    let tmp = TempDir::new()?;
    let path = path_in(&tmp, "export.db")?;
    let mut sink = sink(&path)?;
    sink.write(&batch(vec![usage_row("a", 1, 5)?]))?;
    sink.write(&ExportBatch {
        dataset: Dataset::QuotaSnapshots,
        rows: vec![project::quota_snapshot(
            &machine()?,
            &Versioned {
                revision: Revision::new(1),
                row: quota_sample("2026-05-01T10:00:00Z")?,
            },
        )?],
    })?;

    let conn = Connection::open(&path)?;
    let version: i64 = conn.query_row("SELECT version FROM _schema", [], |r| r.get(0))?;
    assert_eq!(version, i64::from(EXPORT_SCHEMA_VERSION));

    for dataset in [Dataset::UsageEvents, Dataset::QuotaSnapshots] {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{dataset}\")"))?;
        let columns = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let expected = schema(dataset);
        assert_eq!(columns.len(), expected.columns.len());
        for ((name, ty, not_null, pk), column) in columns.iter().zip(expected.columns) {
            assert_eq!(name, &column.sql_name());
            assert_eq!(ty, column.ty.sql());
            assert_eq!(*not_null == 1, !column.nullable || *pk == 1, "{name}");
            assert_eq!(*pk == 1, name == expected.key().name);
        }
    }

    let used: f64 = conn.query_row("SELECT used_percent FROM quota_snapshots", [], |r| r.get(0))?;
    assert!((used - 42.5).abs() < f64::EPSILON);
    // Money is exact integer micros in SQL.
    let (approx, cost): (i64, i64) = conn.query_row(
        "SELECT approximate, cost_reported_usd_micros FROM usage_events",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(approx, 0);
    assert_eq!(cost, 1_500_000);
    Ok(())
}

#[test]
fn dropped_session_ids_are_stored_as_null() -> TestResult {
    let tmp = TempDir::new()?;
    let path = path_in(&tmp, "export.db")?;
    let key = RedactionKey::new([1; 32]);
    let rules = BTreeMap::from([(RedactColumn::SessionId, RedactMode::Drop)]);
    let mut redacted = batch(vec![usage_row("a", 1, 5)?]);
    Redactor::new(&key, &rules).apply(&mut redacted);
    sink(&path)?.write(&redacted)?;
    let conn = Connection::open(&path)?;
    let session: Option<String> =
        conn.query_row("SELECT session_id FROM usage_events", [], |r| r.get(0))?;
    assert_eq!(session, None);
    Ok(())
}

#[test]
fn a_file_of_another_schema_version_is_refused() -> TestResult {
    let tmp = TempDir::new()?;
    let path = path_in(&tmp, "export.db")?;
    {
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE _schema (id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL) STRICT;\
             INSERT INTO _schema VALUES (1, 99);",
        )?;
    }
    let result = sink(&path)?.write(&batch(vec![usage_row("a", 1, 5)?]));
    assert!(matches!(
        result,
        Err(SinkError::SchemaVersion { found: 99, .. })
    ));
    Ok(())
}

#[test]
fn a_refused_directory_fails_with_permission_denied() -> TestResult {
    let tmp = TempDir::new()?;
    let locked = path_in(&tmp, "locked")?;
    fs::create_dir(&locked)?;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500))?;
    let result = sink(&locked.join("export.db"))?.write(&batch(vec![usage_row("a", 1, 5)?]));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700))?;
    assert!(matches!(result, Err(SinkError::PermissionDenied { .. })));
    Ok(())
}
