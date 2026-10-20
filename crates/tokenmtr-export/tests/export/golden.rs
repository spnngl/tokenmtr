//! Encoding goldens, JSONL round trip and redaction.

use std::collections::BTreeMap;

use camino::Utf8Path;
use tokenmtr_config::{FileFormat, RedactColumn, RedactMode};
use tokenmtr_export::{
    ExportBatch, ExportRow, Redactor, RowEncoder, Value, hash_value, project, schema,
};
use tokenmtr_store::{Dataset, RedactionKey, Revision, Versioned};

use crate::fixtures::{
    TestResult, configured, extra_usage_sample, fx_rate_row, live, machine, quota_sample,
    subscription, tool_call, usage,
};

fn versioned<T>(row: T) -> Versioned<T> {
    Versioned {
        revision: Revision::new(7),
        row,
    }
}

/// One fully populated row per dataset.
fn sample_rows(dataset: Dataset) -> TestResult<Vec<ExportRow>> {
    let machine = machine()?;
    Ok(match dataset {
        Dataset::UsageEvents => vec![
            project::usage_event(
                &machine,
                &versioned(live(usage(
                    &machine,
                    "s1",
                    "2026-05-01T10:00:00Z",
                    Some("/work/acme"),
                )?)),
            ),
            project::usage_event(
                &machine,
                &versioned(live(usage(&machine, "s2", "2026-05-01T10:00:01.5Z", None)?)),
            ),
        ],
        Dataset::ToolCalls => vec![project::tool_call(
            &machine,
            &versioned(tool_call("s1", "2026-05-01T10:00:00Z")?),
        )?],
        Dataset::Subscriptions => {
            vec![project::subscription(
                &machine,
                &versioned(configured(subscription()?)),
            )]
        }
        Dataset::QuotaSnapshots => vec![
            project::quota_snapshot(&machine, &versioned(quota_sample("2026-05-01T10:00:00Z")?))?,
            project::quota_snapshot(
                &machine,
                &versioned(extra_usage_sample("2026-05-01T10:00:00Z")?),
            )?,
        ],
        Dataset::FxRates => vec![project::fx_rate(&versioned(fx_rate_row()?))],
    })
}

fn render(format: FileFormat, dataset: Dataset) -> TestResult<String> {
    let encoder = RowEncoder::new(format, dataset);
    let mut bytes = encoder.header()?;
    for row in sample_rows(dataset)? {
        bytes.extend(encoder.encode(&row)?);
    }
    Ok(String::from_utf8(bytes)?)
}

#[test]
fn csv_golden_per_dataset() -> TestResult {
    for dataset in Dataset::ALL {
        let text = render(FileFormat::Csv, dataset)?;
        insta::assert_snapshot!(format!("{dataset}_csv"), text);
    }
    Ok(())
}

#[test]
fn jsonl_golden_per_dataset() -> TestResult {
    for dataset in Dataset::ALL {
        let text = render(FileFormat::Jsonl, dataset)?;
        insta::assert_snapshot!(format!("{dataset}_jsonl"), text);
    }
    Ok(())
}

#[test]
fn csv_header_is_the_schema() -> TestResult {
    for dataset in Dataset::ALL {
        let text = render(FileFormat::Csv, dataset)?;
        let header = text.lines().next().unwrap_or_default();
        let expected: Vec<_> = schema(dataset).names().collect();
        assert_eq!(header, expected.join(","));
        for row in sample_rows(dataset)? {
            assert_eq!(row.values.len(), expected.len(), "{dataset} row width");
        }
    }
    Ok(())
}

#[test]
fn jsonl_round_trips_through_decode_with_typed_values() -> TestResult {
    let encoder = RowEncoder::new(FileFormat::Jsonl, Dataset::UsageEvents);
    let rows = sample_rows(Dataset::UsageEvents)?;
    let mut bytes = Vec::new();
    for row in &rows {
        bytes.extend(encoder.encode(row)?);
    }
    let decoded = encoder.decode(Utf8Path::new("x.jsonl"), &bytes)?;
    assert_eq!(decoded.valid_len, bytes.len());
    let ids: Vec<_> = decoded
        .rows
        .iter()
        .map(|r| (r.id.clone(), r.revision))
        .collect();
    let expected: Vec<_> = rows.iter().map(|r| (r.id(), r.revision.get())).collect();
    assert_eq!(ids, expected);

    let first: serde_json::Value = serde_json::from_slice(&decoded.rows[0].bytes)?;
    assert_eq!(first["revision"], 7);
    assert_eq!(first["approximate"], false);
    assert_eq!(first["output"], 500);
    assert_eq!(first["cost_reported_usd"].to_string(), "1.5");
    assert_eq!(first["at_utc"], "2026-05-01T10:00:00Z");
    assert_eq!(first["cwd"], "/work/acme");
    let second: serde_json::Value = serde_json::from_slice(&decoded.rows[1].bytes)?;
    assert!(second["cwd"].is_null());
    Ok(())
}

#[test]
fn csv_round_trips_through_decode() -> TestResult {
    let encoder = RowEncoder::new(FileFormat::Csv, Dataset::UsageEvents);
    let rows = sample_rows(Dataset::UsageEvents)?;
    let mut bytes = encoder.header()?;
    for row in &rows {
        bytes.extend(encoder.encode(row)?);
    }
    bytes.extend(b"torn,no-newline");
    let decoded = encoder.decode(Utf8Path::new("x.csv"), &bytes)?;
    assert_eq!(decoded.rows.len(), 2);
    assert_eq!(decoded.valid_len, bytes.len() - "torn,no-newline".len());
    Ok(())
}

#[test]
fn a_csv_write_torn_inside_a_quoted_newline_is_cut() -> TestResult {
    let encoder = RowEncoder::new(FileFormat::Csv, Dataset::UsageEvents);
    let machine = machine()?;
    let multiline = |session| -> TestResult<Vec<u8>> {
        let event = usage(&machine, session, "2026-05-01T10:00:00Z", Some("/a\nb"))?;
        Ok(encoder.encode(&project::usage_event(&machine, &versioned(live(event))))?)
    };
    let mut bytes = encoder.header()?;
    bytes.extend(multiline("s1")?);
    let whole = bytes.len();
    let second = multiline("s2")?;
    let cut = second
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("no newline")?;
    // The file now ends with a newline, but inside the quoted `cwd` of the second record.
    bytes.extend(&second[..=cut]);
    let decoded = encoder.decode(Utf8Path::new("x.csv"), &bytes)?;
    assert_eq!(decoded.rows.len(), 1);
    assert_eq!(decoded.valid_len, whole);
    Ok(())
}

#[test]
fn csv_with_a_foreign_header_is_corrupt() {
    let encoder = RowEncoder::new(FileFormat::Csv, Dataset::FxRates);
    let result = encoder.decode(Utf8Path::new("x.csv"), b"a,b\n1,2\n");
    assert!(matches!(
        result,
        Err(tokenmtr_export::SinkError::Corrupt { .. })
    ));
}

fn rules(pairs: &[(RedactColumn, RedactMode)]) -> BTreeMap<RedactColumn, RedactMode> {
    pairs.iter().copied().collect()
}

fn usage_batch() -> TestResult<ExportBatch> {
    Ok(ExportBatch {
        dataset: Dataset::UsageEvents,
        rows: sample_rows(Dataset::UsageEvents)?,
    })
}

fn cell<'a>(batch: &'a ExportBatch, row: usize, column: &str) -> TestResult<&'a Value> {
    let index = schema(batch.dataset)
        .names()
        .position(|n| n == column)
        .ok_or("unknown column")?;
    Ok(&batch.rows[row].values[index])
}

#[test]
fn hash_is_stable_keyed_and_sixteen_hex() -> TestResult {
    let key_a = RedactionKey::new([1; 32]);
    let key_b = RedactionKey::new([2; 32]);
    let a1 = hash_value(&key_a, "/work/acme");
    assert_eq!(a1, hash_value(&key_a, "/work/acme"));
    assert_ne!(a1, hash_value(&key_b, "/work/acme"));
    assert_ne!(a1, hash_value(&key_a, "/work/other"));
    assert_eq!(a1.len(), 16);
    assert!(
        a1.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );

    let rules = rules(&[(RedactColumn::Cwd, RedactMode::Hash)]);
    let mut batch = usage_batch()?;
    Redactor::new(&key_a, &rules).apply(&mut batch);
    assert_eq!(cell(&batch, 0, "cwd")?, &Value::Text(a1));
    assert_eq!(cell(&batch, 1, "cwd")?, &Value::Null, "empty stays empty");
    assert_eq!(
        cell(&batch, 0, "project")?,
        &Value::Text("acme-api".to_owned())
    );

    let mut other = usage_batch()?;
    Redactor::new(&key_b, &rules).apply(&mut other);
    assert_ne!(cell(&batch, 0, "cwd")?, cell(&other, 0, "cwd")?);
    Ok(())
}

#[test]
fn drop_empties_the_cell_and_keeps_the_column() -> TestResult {
    let key = RedactionKey::new([1; 32]);
    let rules = rules(&[
        (RedactColumn::Cwd, RedactMode::Drop),
        (RedactColumn::GitBranch, RedactMode::Drop),
    ]);
    let mut batch = usage_batch()?;
    Redactor::new(&key, &rules).apply(&mut batch);
    assert_eq!(cell(&batch, 0, "cwd")?, &Value::Null);
    assert_eq!(cell(&batch, 0, "git_branch")?, &Value::Null);
    assert_eq!(
        batch.rows[0].values.len(),
        schema(Dataset::UsageEvents).columns.len()
    );

    let encoder = RowEncoder::new(FileFormat::Csv, Dataset::UsageEvents);
    let line = String::from_utf8(encoder.encode(&batch.rows[0])?)?;
    assert!(!line.contains("/work/acme"));
    assert!(!line.contains("feature/export"));
    Ok(())
}

#[test]
fn session_redaction_covers_parent_session_and_tool_calls() -> TestResult {
    let key = RedactionKey::new([1; 32]);
    let rules = rules(&[(RedactColumn::SessionId, RedactMode::Hash)]);
    let mut tools = ExportBatch {
        dataset: Dataset::ToolCalls,
        rows: sample_rows(Dataset::ToolCalls)?,
    };
    Redactor::new(&key, &rules).apply(&mut tools);
    assert_eq!(
        cell(&tools, 0, "session_id")?,
        &Value::Text(hash_value(&key, "s1"))
    );

    let machine = machine()?;
    let mut event = usage(&machine, "child", "2026-05-01T10:00:00Z", None)?;
    event.parent_session = Some(tokenmtr_core::SessionId::try_from("parent")?);
    let mut batch = ExportBatch {
        dataset: Dataset::UsageEvents,
        rows: vec![project::usage_event(&machine, &versioned(live(event)))],
    };
    Redactor::new(&key, &rules).apply(&mut batch);
    assert_eq!(
        cell(&batch, 0, "parent_session_id")?,
        &Value::Text(hash_value(&key, "parent"))
    );
    Ok(())
}

#[test]
fn skill_redaction_applies_to_tool_calls() -> TestResult {
    let key = RedactionKey::new([1; 32]);
    let tools = || -> TestResult<ExportBatch> {
        Ok(ExportBatch {
            dataset: Dataset::ToolCalls,
            rows: sample_rows(Dataset::ToolCalls)?,
        })
    };
    let mut hashed = tools()?;
    Redactor::new(&key, &rules(&[(RedactColumn::Skill, RedactMode::Hash)])).apply(&mut hashed);
    assert_eq!(
        cell(&hashed, 0, "skill")?,
        &Value::Text(hash_value(&key, "git-commit"))
    );
    let mut dropped = tools()?;
    Redactor::new(&key, &rules(&[(RedactColumn::Skill, RedactMode::Drop)])).apply(&mut dropped);
    assert_eq!(cell(&dropped, 0, "skill")?, &Value::Null);
    Ok(())
}

#[test]
fn account_redaction_applies_to_quota_snapshots() -> TestResult {
    let key = RedactionKey::new([1; 32]);
    let rules = rules(&[(RedactColumn::Account, RedactMode::Hash)]);
    let mut batch = ExportBatch {
        dataset: Dataset::QuotaSnapshots,
        rows: sample_rows(Dataset::QuotaSnapshots)?,
    };
    Redactor::new(&key, &rules).apply(&mut batch);
    let text = format!("{batch:?}");
    assert!(!text.contains("me@example.com"));
    assert_eq!(
        cell(&batch, 0, "account")?,
        &Value::Text(hash_value(&key, "me@example.com"))
    );
    Ok(())
}
