//! File sink behaviour: append, snapshot, partitions, durability and refused paths.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;
use tokenmtr_config::{FileFormat, FileMode, Partition};
use tokenmtr_core::SinkId;
use tokenmtr_export::{
    ExportBatch, ExportRow, FileSink, FileTarget, RowEncoder, Sink, SinkError, project,
};
use tokenmtr_store::{Dataset, Revision, Versioned};

use crate::fixtures::{TestResult, machine, path_in, subscription, usage};

fn sink(
    dir: &Utf8Path,
    format: FileFormat,
    mode: FileMode,
    partition: Partition,
) -> TestResult<FileSink> {
    Ok(FileSink::new(
        SinkId::try_from("files")?,
        FileTarget {
            format,
            dir: dir.to_owned(),
            mode,
            partition,
        },
    ))
}

fn usage_row(session: &str, at: &str, revision: u64, output: u64) -> TestResult<ExportRow> {
    let machine = machine()?;
    let mut event = usage(&machine, session, at, Some("/work/acme"))?;
    event.tokens.output = output;
    Ok(project::usage_event(
        &machine,
        &Versioned {
            revision: Revision::new(revision),
            row: event,
        },
    ))
}

fn batch(rows: Vec<ExportRow>) -> ExportBatch {
    ExportBatch {
        dataset: Dataset::UsageEvents,
        rows,
    }
}

fn lines(path: &Utf8Path) -> TestResult<Vec<String>> {
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// Highest revision per id of an export file, as a consumer would compute it.
fn current(path: &Utf8Path, format: FileFormat) -> TestResult<BTreeMap<String, (u64, Vec<u8>)>> {
    let encoder = RowEncoder::new(format, Dataset::UsageEvents);
    let decoded = encoder.decode(path, &fs::read(path)?)?;
    let mut map: BTreeMap<String, (u64, Vec<u8>)> = BTreeMap::new();
    for row in decoded.rows {
        if map.get(&row.id).is_none_or(|(rev, _)| *rev < row.revision) {
            map.insert(row.id, (row.revision, row.bytes));
        }
    }
    Ok(map)
}

fn root(dir: &TempDir) -> TestResult<Utf8PathBuf> {
    path_in(dir, "exports")
}

#[test]
fn append_redelivery_never_duplicates_rows() -> TestResult {
    for format in [FileFormat::Csv, FileFormat::Jsonl] {
        let tmp = TempDir::new()?;
        let dir = root(&tmp)?;
        let mut sink = sink(&dir, format, FileMode::Append, Partition::Month)?;
        let first = batch(vec![
            usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?,
            usage_row("b", "2026-05-02T10:00:00Z", 2, 6)?,
        ]);
        sink.write(&first)?;
        sink.write(&first)?;
        let ext = if format == FileFormat::Csv {
            "csv"
        } else {
            "jsonl"
        };
        let path = dir.join(format!("usage_events-2026-05.{ext}"));
        let expected = if format == FileFormat::Csv { 3 } else { 2 };
        assert_eq!(
            lines(&path)?.len(),
            expected,
            "{format:?}: header once, rows once"
        );

        // A fresh sink (daemon restart) has no cache and must reach the same conclusion.
        let mut restarted = sink_again(&dir, format)?;
        restarted.write(&first)?;
        assert_eq!(lines(&path)?.len(), expected);
    }
    Ok(())
}

fn sink_again(dir: &Utf8Path, format: FileFormat) -> TestResult<FileSink> {
    sink(dir, format, FileMode::Append, Partition::Month)
}

#[test]
fn append_adds_changed_rows_with_their_revision() -> TestResult {
    let tmp = TempDir::new()?;
    let dir = root(&tmp)?;
    let mut sink = sink(&dir, FileFormat::Jsonl, FileMode::Append, Partition::Month)?;
    sink.write(&batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?]))?;
    sink.write(&batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 2, 9)?]))?;
    let path = dir.join("usage_events-2026-05.jsonl");
    assert_eq!(lines(&path)?.len(), 2);
    let map = current(&path, FileFormat::Jsonl)?;
    assert_eq!(map.len(), 1);
    assert_eq!(map.values().next().map(|(rev, _)| *rev), Some(2));
    Ok(())
}

#[test]
fn append_cuts_a_torn_last_line_before_continuing() -> TestResult {
    let tmp = TempDir::new()?;
    let dir = root(&tmp)?;
    let mut first = sink(&dir, FileFormat::Jsonl, FileMode::Append, Partition::Month)?;
    first.write(&batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?]))?;
    let path = dir.join("usage_events-2026-05.jsonl");
    let mut torn = fs::read(&path)?;
    torn.extend(b"{\"event_id\":\"abc\",\"revi");
    fs::write(&path, torn)?;

    let mut restarted = sink(&dir, FileFormat::Jsonl, FileMode::Append, Partition::Month)?;
    restarted.write(&batch(vec![
        usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?,
        usage_row("b", "2026-05-01T11:00:00Z", 2, 6)?,
    ]))?;
    assert_eq!(current(&path, FileFormat::Jsonl)?.len(), 2);
    assert_eq!(lines(&path)?.len(), 2);
    Ok(())
}

#[test]
fn rescans_when_the_file_was_replaced_behind_its_back() -> TestResult {
    let tmp = TempDir::new()?;
    let dir = root(&tmp)?;
    let mut sink = sink(&dir, FileFormat::Csv, FileMode::Append, Partition::All)?;
    let rows = batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?]);
    sink.write(&rows)?;
    let path = dir.join("usage_events-all.csv");
    fs::remove_file(&path)?;
    sink.write(&rows)?;
    assert_eq!(lines(&path)?.len(), 2, "header and the row were rewritten");
    Ok(())
}

#[test]
fn partitions_roll_over_by_month_day_and_all() -> TestResult {
    let rows = || -> TestResult<ExportBatch> {
        Ok(batch(vec![
            usage_row("a", "2026-04-30T23:59:59Z", 1, 1)?,
            usage_row("b", "2026-05-01T00:00:00Z", 2, 2)?,
            usage_row("c", "2026-05-01T12:00:00Z", 3, 3)?,
        ]))
    };
    let names = |dir: &Utf8Path| -> TestResult<Vec<String>> {
        let mut names: Vec<String> = fs::read_dir(dir)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, _>>()?;
        names.sort();
        Ok(names)
    };
    for (partition, expected) in [
        (
            Partition::Month,
            vec!["usage_events-2026-04.csv", "usage_events-2026-05.csv"],
        ),
        (
            Partition::Day,
            vec!["usage_events-2026-04-30.csv", "usage_events-2026-05-01.csv"],
        ),
        (Partition::All, vec!["usage_events-all.csv"]),
    ] {
        let tmp = TempDir::new()?;
        let dir = root(&tmp)?;
        sink(&dir, FileFormat::Csv, FileMode::Append, partition)?.write(&rows()?)?;
        assert_eq!(names(&dir)?, expected, "{partition:?}");
    }
    Ok(())
}

#[test]
fn rollover_keeps_each_header_and_old_files_untouched() -> TestResult {
    let tmp = TempDir::new()?;
    let dir = root(&tmp)?;
    let mut sink = sink(&dir, FileFormat::Csv, FileMode::Append, Partition::Month)?;
    sink.write(&batch(vec![usage_row("a", "2026-04-30T23:59:59Z", 1, 1)?]))?;
    let april = dir.join("usage_events-2026-04.csv");
    let before = fs::read(&april)?;
    sink.write(&batch(vec![usage_row("b", "2026-05-01T00:00:00Z", 2, 2)?]))?;
    assert_eq!(fs::read(&april)?, before);
    let may = lines(&dir.join("usage_events-2026-05.csv"))?;
    assert!(may[0].starts_with("event_id,revision,"));
    assert_eq!(may.len(), 2);
    Ok(())
}

#[test]
fn subscriptions_use_one_file_whatever_the_partition() -> TestResult {
    let tmp = TempDir::new()?;
    let dir = root(&tmp)?;
    let machine = machine()?;
    let row = project::subscription(
        &machine,
        &Versioned {
            revision: Revision::new(1),
            row: subscription()?,
        },
    );
    sink(&dir, FileFormat::Csv, FileMode::Append, Partition::Day)?.write(&ExportBatch {
        dataset: Dataset::Subscriptions,
        rows: vec![row],
    })?;
    assert!(dir.join("subscriptions-all.csv").exists());
    Ok(())
}

#[test]
fn snapshot_holds_one_row_per_id_at_the_highest_revision() -> TestResult {
    for format in [FileFormat::Csv, FileFormat::Jsonl] {
        let tmp = TempDir::new()?;
        let dir = root(&tmp)?;
        let mut sink = sink(&dir, format, FileMode::Snapshot, Partition::Month)?;
        sink.write(&batch(vec![
            usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?,
            usage_row("b", "2026-05-02T10:00:00Z", 2, 6)?,
        ]))?;
        sink.write(&batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 3, 50)?]))?;
        // Redelivering an older revision of `a` changes nothing.
        sink.write(&batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?]))?;
        let ext = if format == FileFormat::Csv {
            "csv"
        } else {
            "jsonl"
        };
        let path = dir.join(format!("usage_events-2026-05.{ext}"));
        let map = current(&path, format)?;
        let revisions: Vec<u64> = map.values().map(|(r, _)| *r).collect();
        let total = lines(&path)?.len() - usize::from(format == FileFormat::Csv);
        assert_eq!(total, 2, "{format:?}: no duplicate ids");
        let mut sorted = revisions;
        sorted.sort_unstable();
        assert_eq!(sorted, vec![2, 3]);
    }
    Ok(())
}

#[test]
fn snapshot_redelivery_leaves_the_file_byte_identical() -> TestResult {
    let tmp = TempDir::new()?;
    let dir = root(&tmp)?;
    let mut sink = sink(&dir, FileFormat::Csv, FileMode::Snapshot, Partition::Month)?;
    let rows = batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?]);
    sink.write(&rows)?;
    let path = dir.join("usage_events-2026-05.csv");
    let before = fs::read(&path)?;
    sink.write(&rows)?;
    assert_eq!(fs::read(&path)?, before);
    Ok(())
}

#[test]
fn existing_file_with_another_schema_is_refused_not_extended() -> TestResult {
    let tmp = TempDir::new()?;
    let dir = root(&tmp)?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("usage_events-2026-05.csv");
    fs::write(&path, "wrong,header\n1,2\n")?;
    let mut sink = sink(&dir, FileFormat::Csv, FileMode::Append, Partition::Month)?;
    let result = sink.write(&batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?]));
    assert!(matches!(result, Err(SinkError::Corrupt { .. })));
    assert_eq!(fs::read_to_string(&path)?, "wrong,header\n1,2\n");
    Ok(())
}

#[test]
fn a_refused_directory_fails_with_permission_denied_and_a_hint() -> TestResult {
    let tmp = TempDir::new()?;
    let locked = path_in(&tmp, "locked")?;
    fs::create_dir(&locked)?;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500))?;
    let dir = locked.join("exports");
    let result = sink(&dir, FileFormat::Csv, FileMode::Append, Partition::Month)?
        .write(&batch(vec![usage_row("a", "2026-05-01T10:00:00Z", 1, 5)?]));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700))?;
    let error = result
        .err()
        .ok_or("write must fail in a read-only directory")?;
    assert!(
        matches!(error, SinkError::PermissionDenied { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("Full Disk Access"));
    Ok(())
}
