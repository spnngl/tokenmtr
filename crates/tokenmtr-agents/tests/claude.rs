//! Claude collector tests over the redacted fixtures in `fixtures/claude`.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use proptest::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};
use tokenmtr_agents::{
    AgentSource, ByteOffset, ClaudeSource, ParseBatch, ParseOutcome, SourceFile, claude, discover,
};
use tokenmtr_core::ProfileId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURES: [&str; 12] = [
    "sess-streaming.jsonl",
    "sess-streaming/subagents/agent-a1b2c3.jsonl",
    "sess-sidechain.jsonl",
    "sess-synthetic.jsonl",
    "sess-no-cache-creation.jsonl",
    "sess-half-written.jsonl",
    "sess-tools.jsonl",
    "sess-mcp.jsonl",
    "sess-api-error.jsonl",
    "sess-compact.jsonl",
    "sess-drift.jsonl",
    "sess-v2-1-287.jsonl",
];

fn config_dir() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude")
}

fn project_dir() -> Utf8PathBuf {
    config_dir().join("projects/-home-demo-app")
}

fn source_for(config: &Utf8Path) -> ClaudeSource {
    ClaudeSource::new(vec![claude::profile_for_config_dir(
        ProfileId::default_profile(),
        config,
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
    source: &ClaudeSource,
    path: &Utf8Path,
    start: ByteOffset,
    batch: &mut ParseBatch,
) -> Result<ParseOutcome, Box<dyn std::error::Error>> {
    let file = SourceFile::new(path.to_owned(), ProfileId::default_profile());
    Ok(source.parse(&file, start, batch)?)
}

fn parse_fixture(name: &str) -> Result<(ParseBatch, ParseOutcome), Box<dyn std::error::Error>> {
    let source = source_for(&config_dir());
    let mut batch = ParseBatch::new();
    let outcome = parse_into(
        &source,
        &project_dir().join(name),
        ByteOffset::ZERO,
        &mut batch,
    )?;
    Ok((batch, outcome))
}

fn snapshot_fixture(name: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let (batch, outcome) = parse_fixture(name)?;
    Ok(serde_json::to_value(snapshot(batch, &outcome)?)?)
}

#[test]
fn fixtures_match_snapshots() -> TestResult {
    for name in FIXTURES {
        let value = snapshot_fixture(name)?;
        // `serde_json/preserve_order` may be enabled by another crate in the workspace build.
        insta::with_settings!({sort_maps => true}, {
            insta::assert_json_snapshot!(name.replace(['/', '.'], "_"), value);
        });
    }
    Ok(())
}

#[test]
fn streaming_duplicates_keep_the_last_line() -> TestResult {
    let (batch, _) = parse_fixture("sess-streaming.jsonl")?;
    let events = serde_json::to_value(batch.events())?;
    let events = events.as_array().ok_or("events not an array")?;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["dedup_key"], "msg_s1|req_s1");
    assert_eq!(events[0]["tokens"]["output"], 57);
    assert_eq!(events[1]["dedup_key"], "msg_s2|req_s2");
    Ok(())
}

#[test]
fn subagent_file_folds_into_the_parent_session() -> TestResult {
    let (batch, _) = parse_fixture("sess-streaming/subagents/agent-a1b2c3.jsonl")?;
    let events = serde_json::to_value(batch.events())?;
    let events = events.as_array().ok_or("events not an array")?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["session"], "sess-streaming");
    assert_eq!(events[0]["subagent"], "agent-a1b2c3");
    assert_eq!(events[0]["is_sidechain"], true);
    assert_eq!(events[0]["tokens"]["output"], 21);
    Ok(())
}

#[test]
fn synthetic_model_is_skipped() -> TestResult {
    let (batch, outcome) = parse_fixture("sess-synthetic.jsonl")?;
    assert_eq!(batch.events().len(), 1);
    assert_eq!(outcome.drift.skipped.ignored, 1);
    assert_eq!(outcome.issues.len(), 0);
    Ok(())
}

#[test]
fn missing_cache_creation_falls_back_to_the_5m_bucket() -> TestResult {
    let (batch, _) = parse_fixture("sess-no-cache-creation.jsonl")?;
    let events = serde_json::to_value(batch.events())?;
    assert_eq!(events[0]["tokens"]["cache_write_5m"], 700);
    assert_eq!(events[0]["tokens"]["cache_write_1h"], 0);
    assert_eq!(events[0]["service_tier"], "priority");
    Ok(())
}

/// The platform comes from the model id, the price modifiers from `usage`, and cache writes the
/// 5m/1h split leaves out are 5-minute writes.
#[test]
fn pricing_inputs_follow_the_model_id_and_usage() -> TestResult {
    let line = |n: u32, model: &str, usage: Value| {
        json!({"type": "assistant", "sessionId": "s1", "uuid": format!("u{n}"),
            "requestId": format!("req{n}"), "timestamp": "2026-03-01T00:00:00Z",
            "message": {"id": format!("msg{n}"), "model": model, "content": [], "usage": usage}})
        .to_string()
    };
    let plain = json!({"input_tokens": 1, "output_tokens": 1});
    let text = [
        line(
            0,
            "claude-opus-4-7",
            json!({"input_tokens": 1, "output_tokens": 1,
            "cache_creation_input_tokens": 100, "speed": "fast", "inference_geo": "US",
            "server_tool_use": {"web_search_requests": 2},
            "cache_creation": {"ephemeral_5m_input_tokens": 10, "ephemeral_1h_input_tokens": 30}}),
        ),
        line(1, "us.anthropic.claude-opus-4-7", plain.clone()),
        line(2, "claude-opus-4-1-20250805-v1:0", plain.clone()),
        line(
            3,
            "claude-sonnet-4-5@20250929",
            json!({"input_tokens": 1, "output_tokens": 1,
            "inference_geo": "not_available"}),
        ),
    ]
    .join("\n")
        + "\n";
    let dir = tempfile::tempdir()?;
    let root = Utf8Path::from_path(dir.path()).ok_or("non-UTF-8 temp dir")?;
    let path = root.join("projects/-app/s1.jsonl");
    fs::create_dir_all(path.parent().ok_or("no parent")?)?;
    fs::write(&path, text)?;
    let mut batch = ParseBatch::new();
    parse_into(&source_for(root), &path, ByteOffset::ZERO, &mut batch)?;
    let events = serde_json::to_value(batch.events())?;
    let providers: Vec<&Value> = (0..4).map(|i| &events[i]["provider"]).collect();
    assert_eq!(providers, ["anthropic", "bedrock", "bedrock", "vertex_ai"]);
    assert_eq!(events[0]["tokens"]["cache_write_5m"], 70);
    assert_eq!(events[0]["tokens"]["cache_write_1h"], 30);
    assert_eq!(
        events[0]["modifiers"],
        json!({"fast": true, "inference_geo": "us", "web_search_requests": 2})
    );
    assert_eq!(events[3]["modifiers"]["inference_geo"], Value::Null);
    Ok(())
}

#[test]
fn tool_uses_of_a_multi_line_message_are_kept_once_with_their_error_flag() -> TestResult {
    let (batch, _) = parse_fixture("sess-tools.jsonl")?;
    let calls = serde_json::to_value(batch.tool_calls())?;
    let flags: Vec<(&Value, &Value, &Value)> = calls
        .as_array()
        .ok_or("calls not an array")?
        .iter()
        .map(|c| {
            (
                &c["call"]["tool_use_id"],
                &c["call"]["tool_name"],
                &c["call"]["is_error"],
            )
        })
        .collect();
    assert_eq!(
        flags,
        [
            (&json!("toolu_t1"), &json!("Read"), &json!(false)),
            (&json!("toolu_t2"), &json!("Bash"), &json!(true)),
            (&json!("toolu_t3"), &json!("Edit"), &json!(false)),
            (&json!("toolu_t4"), &json!("Skill"), &json!(false)),
        ]
    );
    assert_eq!(calls[0]["event_key"], "msg_t1|req_t1");
    assert_eq!(calls[1]["event_key"], "msg_t1|req_t1");
    assert_eq!(calls[3]["event_key"], "msg_t2|req_t2");
    let skills: Vec<&Value> = (0..4).map(|i| &calls[i]["call"]["skill"]).collect();
    assert_eq!(
        skills,
        [
            &Value::Null,
            &Value::Null,
            &Value::Null,
            &json!("git-commit")
        ]
    );
    let orphans: Vec<_> = batch.orphan_tool_results().collect();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].tool_use_id, "toolu_earlier");
    assert!(orphans[0].is_error);
    Ok(())
}

#[test]
fn mcp_tool_names_are_split() -> TestResult {
    let (batch, _) = parse_fixture("sess-mcp.jsonl")?;
    let calls = serde_json::to_value(batch.tool_calls())?;
    assert_eq!(calls[0]["call"]["mcp_server"], "github");
    assert_eq!(calls[0]["call"]["tool_name"], "list_issues");
    assert_eq!(calls[1]["call"]["mcp_server"], "tracker");
    assert_eq!(calls[1]["call"]["is_error"], true);
    Ok(())
}

#[test]
fn api_error_and_compaction_lines_become_signals() -> TestResult {
    let (batch, _) = parse_fixture("sess-api-error.jsonl")?;
    assert_eq!(
        serde_json::to_value(batch.signals())?[0]["kind"],
        "api_error"
    );
    assert_eq!(batch.signals().len(), 1);
    assert_eq!(batch.events().len(), 1);
    let (batch, _) = parse_fixture("sess-compact.jsonl")?;
    assert_eq!(
        serde_json::to_value(batch.signals())?[0]["kind"],
        "compaction"
    );
    assert_eq!(batch.signals().len(), 1);
    assert_eq!(batch.events().len(), 2);
    Ok(())
}

#[test]
fn unmappable_line_gives_one_issue_with_a_skeleton_and_no_values() -> TestResult {
    let (batch, outcome) = parse_fixture("sess-drift.jsonl")?;
    assert_eq!(batch.events().len(), 1);
    assert_eq!(outcome.issues.len(), 1);
    let issue = serde_json::to_string(&outcome.issues)?;
    for leaked in ["SECRET", "123456", "654321", "777777", "/home/demo"] {
        assert!(!issue.contains(leaked), "{leaked} leaked into {issue}");
    }
    for key in ["promptTokens", "completionTokens", "tier"] {
        assert!(issue.contains(key), "{key} missing from {issue}");
    }
    Ok(())
}

#[test]
fn the_newest_known_version_parses_without_drift() -> TestResult {
    let (batch, outcome) = parse_fixture("sess-v2-1-287.jsonl")?;
    assert_eq!(batch.events().len(), 1);
    assert_eq!(outcome.issues.len(), 0);
    assert!(outcome.drift.unknown_versions.is_empty());
    assert_eq!(outcome.drift.usage_lines_failed, 0);
    let tokens = &batch.events()[0].tokens;
    assert_eq!(
        (tokens.cache_write_5m, tokens.cache_write_1h, tokens.output),
        (200, 100, 42),
        "`iterations` must not be counted again"
    );
    Ok(())
}

#[test]
fn half_written_last_line_is_not_consumed() -> TestResult {
    let path = project_dir().join("sess-half-written.jsonl");
    let bytes = fs::read(&path)?;
    let complete = bytes
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .map(|(i, _)| i + 1)
        .next_back()
        .ok_or("no newline in fixture")?;
    assert!(complete < bytes.len(), "fixture must end mid-line");
    let (batch, outcome) = parse_fixture("sess-half-written.jsonl")?;
    assert_eq!(outcome.new_offset.get(), u64::try_from(complete)?);
    assert_eq!(batch.events().len(), 2);
    assert_eq!(outcome.issues.len(), 0);
    Ok(())
}

#[test]
fn discovery_finds_every_fixture_including_subagents() {
    let found = discover(&source_for(&config_dir()));
    assert_eq!(found.issues.len(), 0);
    let mut names: Vec<String> = found
        .files
        .iter()
        .filter_map(|f| f.path.strip_prefix(project_dir()).ok())
        .map(|p| p.as_str().to_owned())
        .collect();
    names.sort();
    let mut expected: Vec<String> = FIXTURES.iter().map(|n| (*n).to_owned()).collect();
    expected.sort();
    assert_eq!(names, expected);
}

#[test]
fn missing_roots_are_not_errors() {
    let found = discover(&source_for(Utf8Path::new("/nonexistent/claude")));
    assert_eq!(found.files.len(), 0);
    assert_eq!(found.issues.len(), 0);
}

/// The comparable output of a parse: everything but the drift counters, which differ by design
/// (`no_needle` versus `no_usage`).
fn comparable(batch: ParseBatch, outcome: &ParseOutcome) -> Result<Value, serde_json::Error> {
    serde_json::to_value(snapshot(batch, outcome)?)
}

fn parse_with(source: &ClaudeSource, path: &Utf8Path) -> Result<Value, Box<dyn std::error::Error>> {
    let mut batch = ParseBatch::new();
    let outcome = parse_into(source, path, ByteOffset::ZERO, &mut batch)?;
    Ok(comparable(batch, &outcome)?)
}

#[test]
fn pre_filter_does_not_change_the_output() -> TestResult {
    let with = source_for(&config_dir());
    let without = source_for(&config_dir()).without_prefilter();
    for name in FIXTURES {
        let path = project_dir().join(name);
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
    let source = source_for(&config_dir());
    let dir = tempfile::tempdir()?;
    let root = Utf8Path::from_path(dir.path()).ok_or("non-UTF-8 temp dir")?;
    for name in FIXTURES {
        let whole = parse_with(&source, &project_dir().join(name))?;
        let bytes = fs::read(project_dir().join(name))?;
        let copy = root.join(name);
        fs::create_dir_all(copy.parent().ok_or("no parent")?)?;
        let boundaries = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1);
        for cut in boundaries {
            // First scan sees only the prefix; the second scan resumes at the stored offset.
            fs::write(&copy, &bytes[..cut])?;
            let mut batch = ParseBatch::new();
            let first = parse_into(&source, &copy, ByteOffset::ZERO, &mut batch)?;
            fs::write(&copy, &bytes)?;
            let second = parse_into(&source, &copy, first.new_offset, &mut batch)?;
            // Issues are per parse call; the drift fixture has one in whichever part holds it.
            let mut resumed = second;
            resumed.issues.splice(0..0, first.issues);
            assert_eq!(comparable(batch, &resumed)?, whole, "{name} cut at {cut}");
        }
    }
    Ok(())
}

fn pool() -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = FIXTURES
        .iter()
        .filter_map(|name| fs::read(project_dir().join(name)).ok())
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
        b"{\"type\":\"assistant\",\"message\":{\"usage\":[]}}",
        b"{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\"}]}}",
        b"[1,2,3] tool_use",
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
        let path = Utf8PathBuf::from_path_buf(dir.path().join("sess-random.jsonl"))
            .map_err(|_| TestCaseError::fail("non-UTF-8 temp dir"))?;
        fs::write(&path, &bytes).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let with = parse_with(&source_for(&config_dir()), &path)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let without = parse_with(&source_for(&config_dir()).without_prefilter(), &path)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(with, without);
    }
}

#[test]
fn parse_line_survives_garbage() -> TestResult {
    let file = SourceFile::new("/x/p/s.jsonl".into(), ProfileId::default_profile());
    let ctx = claude::LineContext::new(&file)?;
    for line in [
        &b""[..],
        b"\xff\xfe",
        b"{",
        b"null",
        b"{\"type\":\"assistant\",\"message\":7}",
        b"{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":-1}}}",
        b"{\"message\":{\"content\":[1,\"a\",null,{\"type\":5}]}}",
    ] {
        let mut batch = ParseBatch::new();
        let _ = claude::parse_line(&ctx, line, &mut batch);
        assert_eq!(batch.events().len(), 0);
    }
    Ok(())
}
