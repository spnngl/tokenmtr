//! The cursor pump: delivery, cursors, isolation and crash resume.

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroUsize;

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;
use tokenmtr_config::{FileFormat, FileMode, Partition, RedactColumn, RedactMode};
use tokenmtr_core::{FxRate, SinkId};
use tokenmtr_export::{
    ConfiguredSink, FileTarget, PumpOptions, Redactor, RowEncoder, Sink, SinkKind, SinkSpec,
    SqliteTarget, Target, export_pending, fetch_batch, hash_value,
};
use tokenmtr_store::{Dataset, FxSource, Revision, Store, StoreRead, StoreReader};

use crate::fixtures::{
    TestResult, changed, clock, open_store, path_in, quota_poll, subscription, tool_call, usage,
};

fn spec(id: &str, target: Target) -> TestResult<SinkSpec> {
    Ok(SinkSpec {
        id: SinkId::try_from(id)?,
        interval: jiff::SignedDuration::from_mins(5),
        datasets: Dataset::ALL.to_vec(),
        redact: BTreeMap::new(),
        target,
    })
}

fn files(dir: &Utf8Path, mode: FileMode, partition: Partition) -> Target {
    Target::File(FileTarget {
        format: FileFormat::Csv,
        dir: dir.to_owned(),
        mode,
        partition,
    })
}

fn options(batch: usize) -> PumpOptions {
    PumpOptions {
        batch_size: NonZeroUsize::new(batch).unwrap_or(NonZeroUsize::MIN),
        clock,
    }
}

fn seed(store: &mut Store) -> TestResult {
    let machine = store.machine_id();
    store.upsert_events(&[
        usage(&machine, "a", "2026-04-30T22:00:00Z", Some("/work/acme"))?,
        usage(&machine, "b", "2026-05-01T10:00:00Z", Some("/work/acme"))?,
        usage(&machine, "c", "2026-05-01T11:00:00Z", None)?,
    ])?;
    store.upsert_tool_calls(&[tool_call("a", "2026-05-01T10:00:00Z")?])?;
    store.upsert_subscriptions(&[subscription()?])?;
    store.upsert_quota_snapshots(&[quota_poll("2026-05-01T10:00:00Z")?])?;
    store.upsert_fx_rates(
        FxSource::Ecb,
        &[FxRate {
            date: jiff::civil::date(2026, 4, 30),
            eur_per_usd: tokenmtr_core::EurPerUsd::from_scaled(920_500_000)?,
        }],
    )?;
    Ok(())
}

fn runs(dir: &TempDir, sink: &str) -> TestResult<Vec<(String, i64, Option<String>)>> {
    let reader = StoreReader::open(&path_in(dir, "usage.db")?)?;
    let mut stmt = reader.connection().prepare(
        "SELECT dataset, row_count, error FROM sink_run WHERE sink = ?1 ORDER BY started_at_us, rowid",
    )?;
    let rows = stmt
        .query_map([sink], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Highest revision per id over every `usage_events-*.csv` in `dir`.
fn collapsed(dir: &Utf8Path) -> TestResult<BTreeMap<String, (u64, Vec<u8>)>> {
    let encoder = RowEncoder::new(FileFormat::Csv, Dataset::UsageEvents);
    let mut map: BTreeMap<String, (u64, Vec<u8>)> = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let path = Utf8PathBuf::from_path_buf(entry?.path())
            .map_err(|p| format!("non-UTF-8 {}", p.display()))?;
        if !path
            .file_name()
            .is_some_and(|n| n.starts_with("usage_events-"))
        {
            continue;
        }
        for row in encoder.decode(&path, &fs::read(&path)?)?.rows {
            if map.get(&row.id).is_none_or(|(rev, _)| *rev < row.revision) {
                map.insert(row.id, (row.revision, row.bytes));
            }
        }
    }
    Ok(map)
}

fn line_count(dir: &Utf8Path, name: &str) -> TestResult<usize> {
    Ok(fs::read_to_string(dir.join(name))?.lines().count())
}

#[test]
fn delivers_every_dataset_and_advances_cursors_per_batch() -> TestResult {
    let tmp = TempDir::new()?;
    let mut store = open_store(&tmp)?;
    seed(&mut store)?;
    let out = path_in(&tmp, "out")?;
    let mut sinks = vec![ConfiguredSink::new(spec(
        "files",
        files(&out, FileMode::Append, Partition::Month),
    )?)];

    let outcomes = export_pending(&mut store, &mut sinks, &options(2));
    let outcome = &outcomes[0];
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    assert_eq!(outcome.delivered[&Dataset::UsageEvents], 3);
    assert_eq!(outcome.rows(), 3 + 1 + 1 + 1 + 1);

    let id = SinkId::try_from("files")?;
    for dataset in Dataset::ALL {
        assert_eq!(
            store.sink_cursor(&id, dataset)?,
            store.head_revision(dataset)?,
            "{dataset}"
        );
    }
    // 3 events in batches of 2 and 1, then one run each for the other four datasets.
    let history = runs(&tmp, "files")?;
    assert_eq!(history.len(), 2 + 4);
    assert!(history.iter().all(|(_, _, error)| error.is_none()));
    assert_eq!(history[0], ("usage_events".to_owned(), 2, None::<String>));

    assert!(out.join("usage_events-2026-04.csv").exists());
    assert_eq!(line_count(&out, "usage_events-2026-05.csv")?, 3);
    assert!(out.join("fx_rates-2026-04.csv").exists());
    assert!(out.join("subscriptions-all.csv").exists());

    // Nothing pending: a second run delivers and records nothing.
    let again = export_pending(&mut store, &mut sinks, &options(2));
    assert_eq!(again[0].rows(), 0);
    assert_eq!(runs(&tmp, "files")?.len(), history.len());
    Ok(())
}

#[test]
fn a_failing_sink_does_not_block_a_healthy_one() -> TestResult {
    let tmp = TempDir::new()?;
    let mut store = open_store(&tmp)?;
    seed(&mut store)?;
    let blocker = path_in(&tmp, "not-a-dir")?;
    fs::write(&blocker, "x")?;
    let good_dir = path_in(&tmp, "good")?;
    let mut sinks = vec![
        ConfiguredSink::new(spec(
            "bad",
            files(&blocker.join("out"), FileMode::Append, Partition::Month),
        )?),
        ConfiguredSink::new(spec(
            "good",
            files(&good_dir, FileMode::Append, Partition::Month),
        )?),
    ];

    let outcomes = export_pending(&mut store, &mut sinks, &options(10));
    assert!(outcomes[0].error.is_some());
    assert_eq!(outcomes[0].rows(), 0);
    assert!(outcomes[1].error.is_none());
    assert_eq!(outcomes[1].delivered[&Dataset::UsageEvents], 3);

    let bad = SinkId::try_from("bad")?;
    assert_eq!(
        store.sink_cursor(&bad, Dataset::UsageEvents)?,
        Revision::ZERO
    );
    let history = runs(&tmp, "bad")?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].1, 0);
    assert!(history[0].2.as_deref().is_some_and(|e| !e.is_empty()));

    // Once the path is fixed the same sink catches up from its cursor.
    fs::remove_file(&blocker)?;
    let outcomes = export_pending(&mut store, &mut sinks, &options(10));
    assert!(outcomes[0].error.is_none());
    assert_eq!(outcomes[0].delivered[&Dataset::UsageEvents], 3);
    Ok(())
}

/// Delivers the next batch to `sink` exactly like the pump, then "crashes": no cursor move, no run.
fn deliver_without_ack(store: &Store, sink: &mut ConfiguredSink, dataset: Dataset) -> TestResult {
    let after = store.sink_cursor(&sink.spec.id, dataset)?;
    let mut batch = fetch_batch(store, dataset, after, 100)?;
    Redactor::new(store.redaction_key(), &sink.spec.redact).apply(&mut batch);
    sink.kind.write(&batch)?;
    Ok(())
}

#[test]
fn crash_between_delivery_and_ack_redelivers_idempotently() -> TestResult {
    let tmp = TempDir::new()?;
    let mut store = open_store(&tmp)?;
    seed(&mut store)?;
    let machine = store.machine_id();

    let append_dir = path_in(&tmp, "append")?;
    let snapshot_dir = path_in(&tmp, "snapshot")?;
    let db = path_in(&tmp, "export.db")?;
    let make = || -> TestResult<Vec<ConfiguredSink>> {
        Ok(vec![
            ConfiguredSink::new(spec(
                "append",
                files(&append_dir, FileMode::Append, Partition::Month),
            )?),
            ConfiguredSink::new(spec(
                "snapshot",
                files(&snapshot_dir, FileMode::Snapshot, Partition::Month),
            )?),
            ConfiguredSink::new(spec(
                "db",
                Target::Sqlite(SqliteTarget { path: db.clone() }),
            )?),
        ])
    };

    // Crash 1: every sink got the batch, none was acked.
    let mut sinks = make()?;
    for sink in &mut sinks {
        deliver_without_ack(&store, sink, Dataset::UsageEvents)?;
    }
    // The daemon restarts: fresh sink objects, no caches.
    let mut sinks = make()?;
    let outcomes = export_pending(&mut store, &mut sinks, &options(100));
    assert!(outcomes.iter().all(|o| o.error.is_none()));

    // A row changes (new revision); crash again after delivering it, then recover.
    let revised = changed(usage(
        &machine,
        "b",
        "2026-05-01T10:00:00Z",
        Some("/work/acme"),
    )?);
    store.upsert_events(&[revised])?;
    let mut sinks = make()?;
    for sink in &mut sinks {
        deliver_without_ack(&store, sink, Dataset::UsageEvents)?;
    }
    let mut sinks = make()?;
    let outcomes = export_pending(&mut store, &mut sinks, &options(100));
    assert!(outcomes.iter().all(|o| o.error.is_none()));
    assert_eq!(outcomes[0].delivered[&Dataset::UsageEvents], 1);

    // Append: max revision per id equals the snapshot file; no id twice in the snapshot.
    let from_append = collapsed(&append_dir)?;
    let from_snapshot = collapsed(&snapshot_dir)?;
    assert_eq!(from_append, from_snapshot);
    assert_eq!(from_append.len(), 3);
    let snapshot_rows: usize = ["usage_events-2026-04.csv", "usage_events-2026-05.csv"]
        .iter()
        .map(|n| line_count(&snapshot_dir, n).map(|c| c - 1))
        .sum::<Result<_, _>>()?;
    assert_eq!(snapshot_rows, 3, "snapshot: one row per id");
    // Append kept both revisions of `b` plus the other two rows, none duplicated.
    let append_rows: usize = ["usage_events-2026-04.csv", "usage_events-2026-05.csv"]
        .iter()
        .map(|n| line_count(&append_dir, n).map(|c| c - 1))
        .sum::<Result<_, _>>()?;
    assert_eq!(append_rows, 4);

    // SQLite: exactly one row per id, at the highest revision.
    let conn = rusqlite::Connection::open(&db)?;
    let count: i64 = conn.query_row("SELECT count(*) FROM usage_events", [], |r| r.get(0))?;
    let distinct: i64 = conn.query_row(
        "SELECT count(DISTINCT event_id) FROM usage_events",
        [],
        |r| r.get(0),
    )?;
    assert_eq!((count, distinct), (3, 3));
    let max_revisions: BTreeMap<String, u64> = from_append
        .iter()
        .map(|(id, (rev, _))| (id.clone(), *rev))
        .collect();
    let mut stmt = conn.prepare("SELECT event_id, revision FROM usage_events")?;
    let in_db: BTreeMap<String, u64> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)?.cast_unsigned())))?
        .collect::<Result<_, _>>()?;
    assert_eq!(in_db, max_revisions);
    Ok(())
}

#[test]
fn redacted_sinks_never_contain_the_raw_value_and_group_by_hash() -> TestResult {
    let tmp = TempDir::new()?;
    let mut store = open_store(&tmp)?;
    seed(&mut store)?;
    let out = path_in(&tmp, "redacted")?;
    let mut redacted = spec("redacted", files(&out, FileMode::Append, Partition::All))?;
    redacted.redact = BTreeMap::from([
        (RedactColumn::Cwd, RedactMode::Hash),
        (RedactColumn::GitBranch, RedactMode::Drop),
        (RedactColumn::Account, RedactMode::Hash),
    ]);
    let mut sinks = vec![ConfiguredSink::new(redacted)];
    let outcomes = export_pending(&mut store, &mut sinks, &options(100));
    assert!(outcomes[0].error.is_none());

    let text = fs::read_to_string(out.join("usage_events-all.csv"))?;
    assert!(!text.contains("/work/acme"));
    assert!(!text.contains("feature/export"));
    let hash = hash_value(store.redaction_key(), "/work/acme");
    assert_eq!(
        text.matches(&hash).count(),
        2,
        "both events of the cwd share one hash"
    );
    assert!(!fs::read_to_string(out.join("quota_snapshots-all.csv"))?.contains("me@example.com"));
    Ok(())
}

#[test]
fn a_sqlite_sink_exports_through_the_pump() -> TestResult {
    let tmp = TempDir::new()?;
    let mut store = open_store(&tmp)?;
    seed(&mut store)?;
    let db = path_in(&tmp, "nested/export.db")?;
    let mut sinks = vec![ConfiguredSink::new(spec(
        "db",
        Target::Sqlite(SqliteTarget { path: db.clone() }),
    )?)];
    let outcomes = export_pending(&mut store, &mut sinks, &options(100));
    assert!(outcomes[0].error.is_none(), "{:?}", outcomes[0].error);
    let conn = rusqlite::Connection::open(&db)?;
    for (table, expected) in [
        ("usage_events", 3),
        ("tool_calls", 1),
        ("subscriptions", 1),
        ("quota_snapshots", 1),
        ("fx_rates", 1),
    ] {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        assert_eq!(n, expected, "{table}");
    }
    assert!(matches!(sinks[0].kind, SinkKind::Sqlite(_)));
    assert_eq!(sinks[0].kind.id().as_str(), "db");
    Ok(())
}
