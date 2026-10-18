//! pi collector tests over the invented fixtures in `fixtures/pi`.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use proptest::prelude::*;
use serde::Serialize;
use serde_json::Value;
use tokenmtr_agents::{
    AgentSource, ByteOffset, ParseBatch, ParseIssue, ParseOutcome, PiSource, SourceFile, discover,
    pi,
};
use tokenmtr_core::{EventKind, ProfileId, ProviderId, SessionId, UsdMicros};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MAIN: &str = "2026-03-02T09-00-00-000Z_sess-main.jsonl";
const FORK: &str = "2026-03-02T10-00-00-000Z_sess-fork.jsonl";
const V2: &str = "2026-01-15T08-00-00-000Z_sess-v2.jsonl";
const V1: &str = "2025-11-01T08-00-00-000Z_sess-v1.jsonl";
const V4: &str = "2026-04-01T08-00-00-000Z_sess-v4.jsonl";
const TOOLS: &str = "2026-03-03T08-00-00-000Z_sess-tools.jsonl";
const HALF: &str = "2026-03-04T08-00-00-000Z_sess-half.jsonl";
const DRIFT: &str = "2026-03-05T08-00-00-000Z_sess-drift.jsonl";

const FIXTURES: [&str; 8] = [MAIN, FORK, V2, V1, V4, TOOLS, HALF, DRIFT];

fn agent_dir() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pi")
}

fn session_dir() -> Utf8PathBuf {
    agent_dir().join("sessions/--home-demo-app--")
}

fn source_for(agent_dir: &Utf8Path) -> PiSource {
    PiSource::new(vec![pi::profile_for_agent_dir(
        ProfileId::default_profile(),
        agent_dir,
    )])
}

/// Everything a parse produces, as one JSON-serializable value.
#[derive(Serialize)]
struct Snapshot {
    new_offset: ByteOffset,
    events: Value,
    tool_calls: Value,
    signals: Value,
    orphan_tool_results: Value,
    issues: Value,
}

fn snapshot(batch: ParseBatch, outcome: &ParseOutcome) -> Result<Snapshot, serde_json::Error> {
    let parts = batch.into_parts();
    Ok(Snapshot {
        new_offset: outcome.new_offset,
        events: serde_json::to_value(&parts.events)?,
        tool_calls: serde_json::to_value(&parts.tool_calls)?,
        signals: serde_json::to_value(&parts.signals)?,
        orphan_tool_results: serde_json::to_value(&parts.orphan_tool_results)?,
        issues: serde_json::to_value(&outcome.issues)?,
    })
}

fn parse_into(
    source: &PiSource,
    path: &Utf8Path,
    start: ByteOffset,
    batch: &mut ParseBatch,
) -> Result<ParseOutcome, Box<dyn std::error::Error>> {
    let file = SourceFile::new(path.to_owned(), ProfileId::default_profile());
    Ok(source.parse(&file, start, batch)?)
}

fn parse_fixture(name: &str) -> Result<(ParseBatch, ParseOutcome), Box<dyn std::error::Error>> {
    let source = source_for(&agent_dir());
    let mut batch = ParseBatch::new();
    let outcome = parse_into(
        &source,
        &session_dir().join(name),
        ByteOffset::ZERO,
        &mut batch,
    )?;
    Ok((batch, outcome))
}

#[test]
fn fixtures_match_snapshots() -> TestResult {
    for name in FIXTURES {
        let (batch, outcome) = parse_fixture(name)?;
        let value = serde_json::to_value(snapshot(batch, &outcome)?)?;
        // `serde_json/preserve_order` may be enabled by another crate in the workspace build.
        insta::with_settings!({sort_maps => true}, {
            insta::assert_json_snapshot!(name.replace(['/', '.'], "_"), value);
        });
    }
    Ok(())
}

#[test]
fn every_usage_bearing_entry_kind_maps_to_its_event_kind() -> TestResult {
    let (batch, outcome) = parse_fixture(MAIN)?;
    let kinds: Vec<(String, EventKind)> = batch
        .events()
        .iter()
        .map(|e| (e.dedup_key.to_string(), e.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("msg_main_1".to_owned(), EventKind::Message),
            (
                "2026-03-02T09:00:06.000Z|tool_result|||50|0|0|0|20|0|200".to_owned(),
                EventKind::ToolResult
            ),
            (
                "2026-03-02T09:00:10.000Z|message|openai|gpt-5|200|1000|0|0|200|64|2400".to_owned(),
                EventKind::Message
            ),
            (
                "2026-03-02T09:00:11.000Z|auxiliary|anthropic|claude-haiku-4-5|30|0|0|0|8|0|70"
                    .to_owned(),
                EventKind::Auxiliary
            ),
            (
                "2026-03-02T09:05:01.000Z|branch_summary|||400|0|0|0|90|0|1300".to_owned(),
                EventKind::BranchSummary
            ),
            ("msg_main_2".to_owned(), EventKind::Message),
            (
                "2026-03-02T09:10:00.000Z|compaction|||900|0|0|0|150|0|2400".to_owned(),
                EventKind::Compaction
            ),
        ]
    );
    // The aborted call (zero usage) is ignored, not drift.
    assert_eq!(outcome.drift.skipped.ignored, 1);
    assert_eq!(outcome.issues.len(), 0);
    assert_eq!(batch.signals().len(), 1);
    Ok(())
}

#[test]
fn cache_split_reasoning_cost_and_model_follow_the_plan() -> TestResult {
    let (batch, _) = parse_fixture(MAIN)?;
    let events = batch.events();
    let first = events.first().ok_or("no events")?;
    // cacheWrite 80 with cacheWrite1h 30: 50 five-minute, 30 one-hour.
    assert_eq!(first.tokens.input_uncached, 120);
    assert_eq!(first.tokens.cache_read, 300);
    assert_eq!(first.tokens.cache_write_5m, 50);
    assert_eq!(first.tokens.cache_write_1h, 30);
    assert_eq!(first.tokens.output, 45);
    assert_eq!(first.tokens.reasoning, 10);
    assert_eq!(first.reported_cost.map(UsdMicros::get), Some(1_500));
    // responseModel wins over model.
    assert_eq!(first.model.as_str(), "claude-sonnet-4-5-20250929");
    assert_eq!(
        first.provider.as_ref().map(ProviderId::as_str),
        Some("anthropic")
    );
    assert_eq!(first.session.as_str(), "sess-main");
    assert_eq!(first.cwd.as_deref(), Some(Utf8Path::new("/home/demo/app")));
    // A tool result carries no model.
    let tool_result = events.get(1).ok_or("no tool result event")?;
    assert_eq!(tool_result.model.as_str(), pi::UNATTRIBUTED_MODEL);
    assert_eq!(tool_result.provider, None);
    Ok(())
}

#[test]
fn old_versions_lack_optional_fields_without_drift() -> TestResult {
    let (batch, outcome) = parse_fixture(V2)?;
    assert_eq!(outcome.issues.len(), 0);
    let events = batch.events();
    assert_eq!(events.len(), 2);
    // No `cost` object: no reported cost, not zero.
    assert_eq!(events[0].reported_cost.map(UsdMicros::get), Some(800));
    assert_eq!(events[1].reported_cost, None);
    assert_eq!(events[1].tokens.cache_write_5m, 15);

    let (batch, outcome) = parse_fixture(V1)?;
    assert_eq!(outcome.issues.len(), 0);
    let events = batch.events();
    // No response id: the entry's immutable content keys it, never its session or entry id.
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].dedup_key.as_str(),
        "2025-11-01T08:00:04.000Z|message|anthropic|claude-3-7-sonnet|70|0|0|0|20|0|500"
    );
    assert_eq!(events[0].session.as_str(), "sess-v1");
    Ok(())
}

#[test]
fn tool_calls_results_and_error_flags() -> TestResult {
    let (batch, outcome) = parse_fixture(TOOLS)?;
    assert_eq!(outcome.issues.len(), 0);
    let calls = serde_json::to_value(batch.tool_calls())?;
    let calls = calls.as_array().ok_or("calls not an array")?;
    let by_id = |id: &str| {
        calls
            .iter()
            .find(|c| c["call"]["tool_use_id"] == id)
            .ok_or(format!("no call {id}"))
    };
    assert_eq!(calls.len(), 4);
    assert_eq!(by_id("call_t1")?["call"]["is_error"], false);
    assert_eq!(by_id("call_t2")?["call"]["is_error"], true);
    let mcp = by_id("call_t3")?;
    assert_eq!(mcp["call"]["tool_name"], "lookup");
    assert_eq!(mcp["call"]["mcp_server"], "demo-server");
    assert_eq!(mcp["event_key"], "msg_tools_1");
    // The aborted message makes no event, so its call has no event to link to.
    assert_eq!(by_id("call_t4")?["event_key"], Value::Null);
    let orphans = serde_json::to_value(batch.orphan_tool_results().collect::<Vec<_>>())?;
    assert_eq!(orphans.as_array().map(Vec::len), Some(1));
    assert_eq!(orphans[0]["tool_use_id"], "call_orphan");
    Ok(())
}

#[test]
fn fork_and_parent_in_one_batch_count_each_entry_once() -> TestResult {
    let source = source_for(&agent_dir());
    let mut batch = ParseBatch::new();
    for name in [MAIN, FORK] {
        parse_into(
            &source,
            &session_dir().join(name),
            ByteOffset::ZERO,
            &mut batch,
        )?;
    }
    // 7 events in the parent, 5 in the fork: 4 copied, 1 new.
    assert_eq!(batch.events().len(), 8);
    let keys: std::collections::BTreeSet<_> = batch
        .events()
        .iter()
        .map(|e| e.dedup_key.to_string())
        .collect();
    assert_eq!(keys.len(), 8);
    assert!(keys.contains("msg_fork_1"));
    // The copied `read` call and the fork's own `bash` call.
    assert_eq!(batch.tool_calls().len(), 2);
    Ok(())
}

/// pi rewrites a v1 file in place when it opens it: every entry gets a random `id` and a
/// `parentId`, the header `version: 3` (pi-mono `migrateV1ToV2`, `migrateV2ToV3`).
#[test]
fn a_v1_file_migrated_to_v3_keeps_its_dedup_keys() -> TestResult {
    let (v1, _) = parse_fixture(V1)?;
    let source = source_for(&agent_dir());
    let mut migrated = ParseBatch::new();
    let outcome = parse_into(
        &source,
        &agent_dir().join("migrated").join(V1),
        ByteOffset::ZERO,
        &mut migrated,
    )?;
    assert!(outcome.issues.is_empty(), "{:?}", outcome.issues);
    let keys = |batch: &ParseBatch| -> Vec<String> {
        batch
            .events()
            .iter()
            .map(|event| event.dedup_key.as_str().to_owned())
            .collect()
    };
    assert_eq!(keys(&v1).len(), 2);
    assert_eq!(keys(&migrated), keys(&v1));
    Ok(())
}

#[test]
fn fork_header_names_its_parent_session() -> TestResult {
    let (batch, _) = parse_fixture(FORK)?;
    for event in batch.events() {
        assert_eq!(event.session.as_str(), "sess-fork");
        assert_eq!(
            event.parent_session.as_ref().map(SessionId::as_str),
            Some("sess-main")
        );
    }
    Ok(())
}

/// pi's own session totals (`usage-totals.ts`): input, output, cacheRead, cacheWrite and
/// `cost.total` summed over assistant and toolResult messages, `usage` entries, and `compaction`
/// and `branch_summary` usage.
#[derive(Debug, Default, PartialEq)]
struct Totals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    /// Micro-dollars.
    cost: i64,
}

fn pi_totals(text: &str) -> Result<Totals, tokenmtr_core::MoneyError> {
    let mut totals = Totals::default();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let usage = match entry["type"].as_str() {
            Some("message")
                if matches!(
                    entry["message"]["role"].as_str(),
                    Some("assistant" | "toolResult")
                ) =>
            {
                &entry["message"]["usage"]
            }
            Some("usage" | "compaction" | "branch_summary") => &entry["usage"],
            _ => continue,
        };
        let count = |key: &str| usage[key].as_u64().unwrap_or(0);
        totals.input += count("input");
        totals.output += count("output");
        totals.cache_read += count("cacheRead");
        totals.cache_write += count("cacheWrite");
        // Each event rounds to whole micro-dollars.
        if let Some(total) = usage["cost"]["total"].as_f64() {
            totals.cost += UsdMicros::from_f64(total)?.get();
        }
    }
    Ok(totals)
}

#[test]
fn reported_totals_equal_pi_totals_per_session() -> TestResult {
    for name in FIXTURES {
        let expected = pi_totals(&fs::read_to_string(session_dir().join(name))?)?;
        let (batch, _) = parse_fixture(name)?;
        let mut actual = Totals::default();
        for event in batch.events() {
            actual.input += event.tokens.input_uncached;
            actual.output += event.tokens.output;
            actual.cache_read += event.tokens.cache_read;
            actual.cache_write += event.tokens.cache_write_5m + event.tokens.cache_write_1h;
            actual.cost += event.reported_cost.map_or(0, UsdMicros::get);
        }
        assert_eq!(actual, expected, "{name}");
    }
    Ok(())
}

#[test]
fn newer_header_version_raises_an_unknown_version_issue_and_still_parses() -> TestResult {
    let (batch, outcome) = parse_fixture(V4)?;
    assert_eq!(
        outcome.issues,
        [ParseIssue::UnknownVersion {
            version: "4".to_owned()
        }]
    );
    assert!(outcome.drift.unknown_versions.contains("4"));
    assert_eq!(batch.events().len(), 1);
    for name in [MAIN, V1, V2] {
        let (_, outcome) = parse_fixture(name)?;
        assert!(outcome.drift.unknown_versions.is_empty(), "{name}");
    }
    Ok(())
}

#[test]
fn unmappable_line_gives_one_issue_with_a_skeleton_and_no_values() -> TestResult {
    let (batch, outcome) = parse_fixture(DRIFT)?;
    assert_eq!(batch.events().len(), 1);
    assert_eq!(outcome.issues.len(), 1);
    let issue = serde_json::to_string(&outcome.issues)?;
    for leaked in [
        "SECRET",
        "123456",
        "654321",
        "777777",
        "/home/demo",
        "msg_drift",
    ] {
        assert!(!issue.contains(leaked), "{leaked} leaked into {issue}");
    }
    for key in ["promptTokens", "completionTokens", "cachedTokens", "tier"] {
        assert!(issue.contains(key), "{key} missing from {issue}");
    }
    Ok(())
}

#[test]
fn half_written_last_line_is_not_consumed() -> TestResult {
    let bytes = fs::read(session_dir().join(HALF))?;
    let complete = bytes
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .map(|(i, _)| i + 1)
        .next_back()
        .ok_or("no newline in fixture")?;
    assert!(complete < bytes.len(), "fixture must end mid-line");
    let (batch, outcome) = parse_fixture(HALF)?;
    assert_eq!(outcome.new_offset.get(), u64::try_from(complete)?);
    assert_eq!(batch.events().len(), 2);
    assert_eq!(outcome.issues.len(), 0);
    Ok(())
}

#[test]
fn discovery_finds_every_fixture() {
    let found = discover(&source_for(&agent_dir()));
    assert_eq!(found.issues.len(), 0);
    let mut names: Vec<String> = found
        .files
        .iter()
        .filter_map(|f| f.path.strip_prefix(session_dir()).ok())
        .map(|p| p.as_str().to_owned())
        .collect();
    names.sort();
    let mut expected: Vec<String> = FIXTURES.iter().map(|n| (*n).to_owned()).collect();
    expected.sort();
    assert_eq!(names, expected);
}

#[test]
fn missing_roots_are_not_errors() {
    let found = discover(&source_for(Utf8Path::new("/nonexistent/pi")));
    assert_eq!(found.files.len(), 0);
    assert_eq!(found.issues.len(), 0);
}

/// The comparable output of a parse: everything but the drift counters, which differ by design
/// (`no_needle` versus `no_usage`).
fn comparable(batch: ParseBatch, outcome: &ParseOutcome) -> Result<Value, serde_json::Error> {
    serde_json::to_value(snapshot(batch, outcome)?)
}

fn parse_with(source: &PiSource, path: &Utf8Path) -> Result<Value, Box<dyn std::error::Error>> {
    let mut batch = ParseBatch::new();
    let outcome = parse_into(source, path, ByteOffset::ZERO, &mut batch)?;
    Ok(comparable(batch, &outcome)?)
}

#[test]
fn pre_filter_does_not_change_the_output() -> TestResult {
    let with = source_for(&agent_dir());
    let without = source_for(&agent_dir()).without_prefilter();
    for name in FIXTURES {
        let path = session_dir().join(name);
        assert_eq!(
            parse_with(&with, &path)?,
            parse_with(&without, &path)?,
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn parsing_from_a_line_boundary_equals_parsing_the_whole_file() -> TestResult {
    let source = source_for(&agent_dir());
    let dir = tempfile::tempdir()?;
    let root = Utf8Path::from_path(dir.path()).ok_or("non-UTF-8 temp dir")?;
    for name in FIXTURES {
        let whole = parse_with(&source, &session_dir().join(name))?;
        let bytes = fs::read(session_dir().join(name))?;
        // The copy keeps the fixture's file name: it is the fallback session id.
        let copy = root.join(name);
        let boundaries = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1);
        for cut in boundaries {
            // First scan sees only the prefix; the second scan resumes at the stored offset, where
            // the header is no longer part of the scan.
            fs::write(&copy, &bytes[..cut])?;
            let mut batch = ParseBatch::new();
            let first = parse_into(&source, &copy, ByteOffset::ZERO, &mut batch)?;
            fs::write(&copy, &bytes)?;
            let second = parse_into(&source, &copy, first.new_offset, &mut batch)?;
            // Issues are per parse call: a header issue shows in both, a drift issue in the part
            // that holds the line.
            let mut resumed = second;
            resumed.issues.splice(0..0, first.issues);
            resumed.issues.dedup();
            assert_eq!(comparable(batch, &resumed)?, whole, "{name} cut at {cut}");
        }
    }
    Ok(())
}

fn pool() -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = FIXTURES
        .iter()
        .filter_map(|name| fs::read(session_dir().join(name)).ok())
        .flat_map(|bytes| {
            bytes
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>()
        })
        .collect();
    for junk in [
        &b"{}"[..],
        b"not json at all usage",
        b"{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"usage\":[]}}",
        b"{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"c\"}}",
        b"{\"type\":\"compaction\",\"timestamp\":\"2026-03-02T09:10:00.000Z\"}",
        b"[1,2,3] toolCall",
    ] {
        lines.push(junk.to_vec());
    }
    lines
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn pre_filter_equivalence_over_random_line_subsets(
        mask in prop::collection::vec(any::<bool>(), pool().len()),
        trailing_newline in any::<bool>(),
    ) {
        let lines = pool();
        let mut bytes = Vec::new();
        for (line, keep) in lines.iter().zip(&mask) {
            if *keep {
                bytes.extend_from_slice(line);
                bytes.push(b'\n');
            }
        }
        if !trailing_newline {
            bytes.pop();
        }
        let dir = tempfile::tempdir().map_err(|e| TestCaseError::fail(e.to_string()))?;
        let path = Utf8PathBuf::from_path_buf(
            dir.path().join("2026-03-06T08-00-00-000Z_sess-random.jsonl"),
        )
        .map_err(|_| TestCaseError::fail("non-UTF-8 temp dir"))?;
        fs::write(&path, &bytes).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let with = parse_with(&source_for(&agent_dir()), &path)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let without = parse_with(&source_for(&agent_dir()).without_prefilter(), &path)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(with, without);
    }
}

#[test]
fn a_negative_reported_cost_is_dropped_and_flags_the_event_approximate() -> TestResult {
    let file = SourceFile::new("/x/p/s.jsonl".into(), ProfileId::default_profile());
    let ctx = pi::LineContext::new(&file, None)?;
    let line = br#"{"type":"usage","timestamp":"2026-03-02T09:00:00.000Z","provider":"anthropic","model":"claude-haiku-4-5","usage":{"input":10,"output":5,"cost":{"total":-0.5}}}"#;
    let mut batch = ParseBatch::new();
    let _ = pi::parse_line(&ctx, line, &mut batch);
    let event = batch.events().first().ok_or("no event")?;
    assert_eq!(event.reported_cost, None);
    assert!(event.approximate);
    Ok(())
}

#[test]
fn parse_line_survives_garbage() -> TestResult {
    let file = SourceFile::new("/x/p/s.jsonl".into(), ProfileId::default_profile());
    let ctx = pi::LineContext::new(&file, None)?;
    for line in [
        &b""[..],
        b"\xff\xfe",
        b"{",
        b"null",
        b"{\"type\":\"message\",\"message\":7}",
        b"{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":-1}}}",
        b"{\"type\":\"usage\",\"usage\":{\"input\":1,\"cost\":{\"total\":\"x\"}}}",
        b"{\"message\":{\"content\":[1,\"a\",null,{\"type\":5}]}}",
    ] {
        let mut batch = ParseBatch::new();
        let _ = pi::parse_line(&ctx, line, &mut batch);
        assert_eq!(batch.events().len(), 0);
    }
    Ok(())
}
