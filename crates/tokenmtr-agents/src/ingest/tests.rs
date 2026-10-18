//! Engine tests: every Done-when line of Phase 6 that concerns scanning, on temp dirs and a real
//! store. Timestamps in transcripts are explicit UTC and the display zone is UTC.

use std::fs;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::tz::TimeZone;
use serde_json::json;
use tempfile::TempDir;
use tokenmtr_core::{AgentId, ProfileId, UsageEvent};
use tokenmtr_store::{Dataset, IssueKind, Revision, Store, StoreRead as _, Versioned};

use super::{
    IngestError, IngestSettings, Ingestor, ParseReason, ScanMode, ScanPriority, ScanReport,
    ScanRequest,
};
use crate::source::Profile;
use crate::testkit::{rules, utf8_dir};
use crate::{AgentKind, ClaudeSource, PiSource};

const SONNET_V1: &str = r#"{"claude-sonnet-4-5": {"litellm_provider":"anthropic","mode":"chat",
    "input_cost_per_token":3e-06,"output_cost_per_token":1.5e-05,
    "cache_read_input_token_cost":3e-07,"cache_creation_input_token_cost":3.75e-06}}"#;

const SONNET_AND_OPUS_V2: &str = r#"{"claude-sonnet-4-5": {"litellm_provider":"anthropic","mode":"chat",
    "input_cost_per_token":6e-06,"output_cost_per_token":3e-05},
    "claude-test-newer": {"litellm_provider":"anthropic","mode":"chat",
    "input_cost_per_token":1.5e-05,"output_cost_per_token":7.5e-05}}"#;

/// A temp machine: agent roots, a store, an ingestor.
struct Machine {
    _dir: TempDir,
    root: Utf8PathBuf,
    store: Store,
    ingestor: Ingestor,
}

impl Machine {
    fn new() -> Self {
        Self::with_rules("")
    }

    fn with_rules(rules_toml: &str) -> Self {
        Self::with_privacy(rules_toml, false)
    }

    fn with_privacy(rules_toml: &str, redact_paths: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8_dir(&dir);
        let store = Store::open(&root.join("store.db")).unwrap();
        let ingestor = ingestor(&root, rules_toml, redact_paths);
        Self {
            _dir: dir,
            root,
            store,
            ingestor,
        }
    }

    fn claude_dir(&self) -> Utf8PathBuf {
        self.root.join("claude").join("projects").join("-app")
    }

    fn pi_dir(&self) -> Utf8PathBuf {
        self.root.join("pi").join("sessions").join("--app--")
    }

    /// Replaces the ingestor, as a daemon restart with a changed config would.
    fn reconfigure(&mut self, rules_toml: &str) {
        self.ingestor = ingestor(&self.root, rules_toml, false);
    }

    fn scan(&mut self) -> ScanReport {
        self.scan_with(&ScanRequest::incremental(ScanPriority::UserInitiated))
    }

    fn scan_with(&mut self, request: &ScanRequest) -> ScanReport {
        self.ingestor.scan(&mut self.store, request).unwrap()
    }

    fn head(&self) -> Revision {
        self.store.head_revision(Dataset::UsageEvents).unwrap()
    }

    fn events(&self) -> Vec<Versioned<UsageEvent>> {
        let mut events = self
            .store
            .usage_events_after(Revision::ZERO, 100_000)
            .unwrap();
        events.sort_by(|a, b| a.row.dedup_key.as_str().cmp(b.row.dedup_key.as_str()));
        events
    }

    fn snapshot(&self) -> Vec<Snapshot> {
        self.events()
            .into_iter()
            .map(|e| Snapshot::of(&e.row))
            .collect()
    }
}

fn ingestor(root: &Utf8Path, rules_toml: &str, redact_paths: bool) -> Ingestor {
    let claude = Profile::new(
        ProfileId::default_profile(),
        vec![root.join("claude").join("projects")],
    );
    let pi = Profile::new(
        ProfileId::default_profile(),
        vec![root.join("pi").join("sessions")],
    );
    let settings = IngestSettings {
        sources: vec![
            AgentKind::Claude(ClaudeSource::new(vec![claude])),
            AgentKind::Pi(PiSource::new(vec![pi])),
        ],
        rules: rules(rules_toml),
        overrides: Vec::new(),
        zone: TimeZone::UTC,
        import_claude_stats_cache: true,
        protected: vec![root.join("protected")],
        redact_paths,
    };
    Ingestor::open(settings, &root.join("store.db")).unwrap()
}

/// What two stores must agree on to hold "the same events" (ids differ per machine).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    key: String,
    at_us: i64,
    input: u64,
    output: u64,
    estimate: Option<i64>,
    project: Option<String>,
}

impl Snapshot {
    fn of(event: &UsageEvent) -> Self {
        Self {
            key: event.dedup_key.as_str().to_owned(),
            at_us: event.at.as_microsecond(),
            input: event.tokens.input_uncached,
            output: event.tokens.output,
            estimate: event.cost.estimated().map(|e| e.usd.get()),
            project: event.project.as_ref().map(|p| p.as_str().to_owned()),
        }
    }
}

fn stamp(n: u32) -> String {
    format!(
        "2026-03-07T{:02}:{:02}:{:02}.000Z",
        1 + n / 3600,
        n / 60 % 60,
        n % 60
    )
}

fn claude_line(n: u32, session: &str, cwd: &str, model: &str, input: u64, output: u64) -> String {
    json!({
        "parentUuid": null, "isSidechain": false, "userType": "external", "cwd": cwd,
        "sessionId": session, "version": "2.1.0", "gitBranch": "main", "type": "assistant",
        "message": {
            "id": format!("msg_{session}_{n}"), "type": "message", "role": "assistant",
            "model": model, "content": [{"type": "text", "text": "x"}], "stop_reason": null,
            "usage": {
                "input_tokens": input, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0},
                "output_tokens": output, "service_tier": "standard"
            }
        },
        "requestId": format!("req_{session}_{n}"), "uuid": format!("u-{session}-{n}"),
        "timestamp": stamp(n)
    })
    .to_string()
}

fn sonnet(n: u32, session: &str, cwd: &str) -> String {
    claude_line(n, session, cwd, "claude-sonnet-4-5", 100 + u64::from(n), 10)
}

fn lines(items: &[String]) -> String {
    let mut text = String::new();
    for item in items {
        text.push_str(item);
        text.push('\n');
    }
    text
}

fn write(path: &Utf8Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn append(path: &Utf8Path, body: &str) {
    use std::io::Write as _;
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(body.as_bytes()).unwrap();
}

fn claude_agent(report: &ScanReport) -> &crate::ingest::AgentScan {
    report.agent(AgentId::CLAUDE).unwrap()
}

fn session_file(
    machine: &Machine,
    session: &str,
    n: std::ops::Range<u32>,
    cwd: &str,
) -> Utf8PathBuf {
    let path = machine.claude_dir().join(format!("{session}.jsonl"));
    let body = lines(&n.map(|i| sonnet(i, session, cwd)).collect::<Vec<_>>());
    write(&path, &body);
    path
}

#[test]
fn a_second_scan_reads_nothing_and_moves_no_revision() {
    let mut machine = Machine::new();
    session_file(&machine, "s1", 0..5, "/work/app");
    let first = machine.scan();
    assert_eq!(claude_agent(&first).events.inserted, 5);
    assert_eq!(claude_agent(&first).files_parsed, 1);
    let head = machine.head();
    let before = machine.snapshot();

    let second = machine.scan();
    assert_eq!(second.bytes_parsed(), 0);
    assert_eq!(claude_agent(&second).files_unchanged, 1);
    assert_eq!(claude_agent(&second).files_parsed, 0);
    assert_eq!(machine.head(), head);

    let full = machine.scan_with(&ScanRequest::full(ScanPriority::UserInitiated));
    assert_eq!(claude_agent(&full).events.changed(), 0);
    assert_eq!(claude_agent(&full).events.unchanged, 5);
    assert_eq!(
        machine.head(),
        head,
        "a full rescan of unchanged files moves nothing"
    );
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn appending_then_scanning_equals_parsing_the_whole_file() {
    let mut incremental = Machine::new();
    let path = session_file(&incremental, "s1", 0..5, "/work/app");
    incremental.scan();
    let consumed = fs::metadata(&path).unwrap().len();
    append(
        &path,
        &lines(
            &(5..9)
                .map(|i| sonnet(i, "s1", "/work/app"))
                .collect::<Vec<_>>(),
        ),
    );
    let report = incremental.scan();
    let file = &claude_agent(&report).files[0];
    assert_eq!((file.reason, file.from), (ParseReason::Appended, consumed));
    assert_eq!(claude_agent(&report).events.inserted, 4);

    let mut whole = Machine::new();
    session_file(&whole, "s1", 0..9, "/work/app");
    whole.scan();
    assert_eq!(incremental.snapshot(), whole.snapshot());
    assert_eq!(incremental.snapshot().len(), 9);
}

#[test]
fn a_half_written_last_line_waits_for_its_newline() {
    let mut machine = Machine::new();
    let path = session_file(&machine, "s1", 0..2, "/work/app");
    let half = sonnet(2, "s1", "/work/app");
    append(&path, &half[..half.len() / 2]);
    assert_eq!(claude_agent(&machine.scan()).events.inserted, 2);
    append(&path, &format!("{}\n", &half[half.len() / 2..]));
    let report = machine.scan();
    assert_eq!(claude_agent(&report).events.inserted, 1);
    assert_eq!(machine.snapshot().len(), 3);
}

#[test]
fn a_truncated_file_is_parsed_again_from_zero() {
    let mut machine = Machine::new();
    let path = session_file(&machine, "s1", 0..6, "/work/app");
    machine.scan();
    write(&path, &lines(&[sonnet(0, "s1", "/work/app")]));
    let report = machine.scan();
    let file = &claude_agent(&report).files[0];
    assert_eq!((file.reason, file.from), (ParseReason::Truncated, 0));
    assert_eq!(claude_agent(&report).events.unchanged, 1);
}

#[test]
fn a_replaced_file_is_parsed_again_from_zero() {
    let mut machine = Machine::new();
    let path = session_file(&machine, "s1", 0..3, "/work/app");
    machine.scan();
    // A new inode at the same path: written next to the old file, then renamed over it.
    let other = machine.claude_dir().join("replacement.tmp");
    write(
        &other,
        &lines(
            &(0..8)
                .map(|i| sonnet(i, "s1", "/work/app"))
                .collect::<Vec<_>>(),
        ),
    );
    fs::rename(&other, &path).unwrap();
    let report = machine.scan();
    let file = &claude_agent(&report).files[0];
    assert_eq!((file.reason, file.from), (ParseReason::Replaced, 0));
    assert_eq!(machine.snapshot().len(), 8);
}

#[test]
fn bytes_changed_before_the_checkpoint_force_a_full_parse() {
    let mut machine = Machine::new();
    let path = session_file(&machine, "s1", 0..3, "/work/app");
    machine.scan();
    // Same length lines, the last consumed one edited in place, and a new line appended.
    let mut body = lines(
        &(0..2)
            .map(|i| sonnet(i, "s1", "/work/app"))
            .collect::<Vec<_>>(),
    );
    body.push_str(&lines(&[
        claude_line(2, "s1", "/work/app", "claude-sonnet-4-5", 999, 10),
        sonnet(3, "s1", "/work/app"),
    ]));
    write(&path, &body);
    let report = machine.scan();
    let file = &claude_agent(&report).files[0];
    assert_eq!((file.reason, file.from), (ParseReason::Rewritten, 0));
    let edited = machine
        .snapshot()
        .into_iter()
        .find(|s| s.key.contains("s1_2"))
        .unwrap();
    assert_eq!(edited.input, 999);
}

#[test]
fn a_crash_before_the_commit_stores_nothing_and_the_rescan_stores_each_row_once() {
    let mut machine = Machine::new();
    session_file(&machine, "s1", 0..6, "/work/app");
    machine.ingestor.crash_before_commit = true;
    let crashed = machine.ingestor.scan(
        &mut machine.store,
        &ScanRequest::incremental(ScanPriority::UserInitiated),
    );
    assert!(matches!(crashed, Err(IngestError::Crash)));
    assert!(
        machine.snapshot().is_empty(),
        "no row without its checkpoint"
    );
    let checkpoints = machine.store.load_checkpoints(AgentId::CLAUDE).unwrap();
    assert_eq!(
        checkpoints[0].checkpoint.offset, 0,
        "the checkpoint did not advance"
    );

    machine.ingestor.crash_before_commit = false;
    let report = machine.scan();
    assert_eq!(claude_agent(&report).files[0].from, 0);
    assert_eq!(claude_agent(&report).events.inserted, 6);
    assert_eq!(machine.snapshot().len(), 6);
    let after = machine.store.load_checkpoints(AgentId::CLAUDE).unwrap();
    assert!(after[0].checkpoint.offset > 0);
}

#[test]
fn a_rule_change_moves_only_the_events_of_the_affected_cwds() {
    let mut machine = Machine::new();
    session_file(&machine, "sa", 0..3, "/work/a/app");
    session_file(&machine, "sb", 0..2, "/work/b/app");
    machine.scan();
    let before = machine.events();
    assert!(before.iter().all(|e| {
        e.row
            .project
            .as_ref()
            .map(tokenmtr_core::ProjectKey::as_str)
            == Some(e.row.cwd.as_ref().unwrap().as_str())
    }));
    let head = machine.head();

    machine
        .reconfigure("[[projects]]\nmatch = \"/work/a/**\"\nname = \"alpha\"\nclient = \"acme\"\n");
    let report = machine.scan();
    assert_eq!(report.cwds_reresolved, 1);
    assert_eq!(report.events_reassigned, 3);
    assert!(machine.head() > head);
    for event in machine.events() {
        let old = before
            .iter()
            .find(|b| b.row.dedup_key == event.row.dedup_key)
            .unwrap();
        if event
            .row
            .cwd
            .as_ref()
            .unwrap()
            .as_str()
            .starts_with("/work/a")
        {
            assert_eq!(event.row.project.as_ref().unwrap().as_str(), "alpha");
            assert_eq!(event.row.client.as_deref(), Some("acme"));
            assert!(event.revision > old.revision);
        } else {
            assert_eq!(
                event.revision, old.revision,
                "other projects keep their revision"
            );
        }
    }

    let head = machine.head();
    let again = machine.scan();
    assert_eq!((again.cwds_reresolved, again.events_reassigned), (0, 0));
    assert_eq!(machine.head(), head);

    machine.reconfigure("");
    let removed = machine.scan();
    assert_eq!(
        removed.cwds_reresolved, 1,
        "dropping the rule gives the cwd its git/cwd answer"
    );
    assert!(machine.events().iter().all(|e| {
        e.row
            .project
            .as_ref()
            .unwrap()
            .as_str()
            .starts_with("/work/")
    }));
}

#[test]
fn redacted_paths_never_reach_the_store_and_projects_still_resolve() {
    let rule = "[[projects]]\nmatch = \"/work/a/**\"\nname = \"alpha\"\nclient = \"acme\"\n";
    let mut machine = Machine::with_privacy(rule, true);
    session_file(&machine, "sa", 0..3, "/work/a/app");
    session_file(&machine, "sb", 0..2, "/definitely/not/here");
    machine.scan();
    let key = machine.store.redaction_key().clone();

    let events = machine.events();
    assert_eq!(events.len(), 5);
    for event in &events {
        let row = &event.row;
        assert_eq!(row.git_branch.as_deref(), Some(key.hash("main").as_str()));
        let cwd = row.cwd.as_ref().unwrap().as_str();
        if cwd == key.hash("/work/a/app") {
            assert_eq!(row.project.as_ref().unwrap().as_str(), "alpha");
            assert_eq!(row.client.as_deref(), Some("acme"));
        } else {
            assert_eq!(cwd, key.hash("/definitely/not/here"));
            assert_eq!(
                row.project.as_ref().unwrap().as_str(),
                key.hash("/definitely/not/here"),
                "a path-derived project key is hashed too"
            );
        }
    }
    let projects = machine.store.projects().unwrap();
    assert_eq!(projects.len(), 2);
    assert!(projects.iter().all(|p| p.repo_root.is_none()));
    assert!(projects.iter().any(|p| p.name == "alpha"));
    let issues = machine.store.open_parse_issues().unwrap();
    let missing: Vec<_> = issues
        .iter()
        .filter(|i| i.kind == IssueKind::MissingCwd)
        .collect();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].subject, key.hash("/definitely/not/here"));

    // Not one byte of the database files (main file and write-ahead log) holds a real cwd.
    // (The configured rule glob `/work/a/**` is stored as the reason of a match: it is config.)
    for entry in fs::read_dir(&machine.root).unwrap() {
        let path = entry.unwrap().path();
        let is_store = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("store.db"));
        if is_store && path.is_file() {
            let bytes = fs::read(&path).unwrap();
            for needle in [&b"/work/a/app"[..], b"definitely/not/here"] {
                let hit = bytes
                    .windows(needle.len())
                    .position(|window| window == needle);
                assert!(
                    hit.is_none(),
                    "{path:?} holds {:?} near {:?}",
                    String::from_utf8_lossy(needle),
                    hit.map(|at| String::from_utf8_lossy(
                        &bytes[at.saturating_sub(80)..(at + 80).min(bytes.len())]
                    )
                    .into_owned())
                );
            }
        }
    }
}

#[test]
fn stored_events_keep_their_pricing_when_a_newer_snapshot_arrives() {
    let mut machine = Machine::new();
    let v1 = machine
        .store
        .put_pricing_snapshot(
            SONNET_V1.as_bytes(),
            "2026-03-01T00:00:00Z".parse().unwrap(),
            Some("test"),
        )
        .unwrap();
    let path = machine.claude_dir().join("s1.jsonl");
    write(
        &path,
        &lines(&[
            claude_line(0, "s1", "/work/app", "claude-sonnet-4-5", 1000, 100),
            claude_line(1, "s1", "/work/app", "claude-test-newer", 1000, 100),
        ]),
    );
    machine.scan();
    let priced = |machine: &Machine| -> Vec<(String, Option<(i64, String)>)> {
        machine
            .events()
            .into_iter()
            .map(|e| {
                (
                    e.row.model.as_str().to_owned(),
                    e.row
                        .cost
                        .estimated()
                        .map(|est| (est.usd.get(), est.pricing_version.as_str().to_owned())),
                )
            })
            .collect()
    };
    let first = priced(&machine);
    assert_eq!(first[1].0, "claude-test-newer");
    assert_eq!(first[1].1, None, "the model has no rate in v1");
    assert_eq!(first[0].1, Some((4500, v1.as_str().to_owned())));

    let v2 = machine
        .store
        .put_pricing_snapshot(
            SONNET_AND_OPUS_V2.as_bytes(),
            "2026-03-02T00:00:00Z".parse().unwrap(),
            Some("test"),
        )
        .unwrap();
    assert_ne!(v1, v2);
    machine.scan_with(&ScanRequest::full(ScanPriority::UserInitiated));
    let second = priced(&machine);
    assert_eq!(
        second[0], first[0],
        "an already priced event is not repriced silently"
    );
    assert_eq!(
        second[1].1,
        Some((22_500, v2.as_str().to_owned())),
        "an unpriced event gets priced"
    );
}

#[test]
fn a_store_without_a_snapshot_gets_the_embedded_one() {
    let mut machine = Machine::new();
    assert!(machine.store.active_pricing_snapshot().unwrap().is_none());
    session_file(&machine, "s1", 0..1, "/work/app");
    machine.scan();
    assert!(machine.store.active_pricing_snapshot().unwrap().is_some());
}

#[test]
fn pi_zero_cost_with_tokens_is_priced_by_the_catalog_and_flagged_approximate() {
    let mut machine = Machine::new();
    machine
        .store
        .put_pricing_snapshot(
            SONNET_V1.as_bytes(),
            "2026-03-01T00:00:00Z".parse().unwrap(),
            Some("test"),
        )
        .unwrap();
    let message = |id: &str, response: &str, total: f64| {
        json!({"type": "message", "id": id, "parentId": null, "timestamp": "2026-03-03T08:00:05.000Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "x"}],
                "api": "anthropic-messages", "provider": "anthropic", "model": "claude-sonnet-4-5",
                "responseId": response,
                "usage": {"input": 1000, "output": 100, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 1100, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                    "cacheWrite": 0, "total": total}},
                "stopReason": "stop", "timestamp": 1_772_524_805_000_i64}})
        .to_string()
    };
    let header = json!({"type": "session", "version": 3, "id": "p1",
        "timestamp": "2026-03-03T08:00:00.000Z", "cwd": "/work/app"})
    .to_string();
    let path = machine.pi_dir().join("2026-03-03T08-00-00-000Z_p1.jsonl");
    write(
        &path,
        &lines(&[
            header,
            message("m1", "resp_zero", 0.0),
            message("m2", "resp_paid", 0.5),
        ]),
    );
    let report = machine.scan();
    assert_eq!(report.agent(AgentId::PI).unwrap().events.inserted, 2);
    let events = machine.events();
    let zero = events
        .iter()
        .find(|e| e.row.dedup_key.as_str().contains("resp_zero"))
        .unwrap();
    assert_eq!(zero.row.cost.reported(), None);
    assert_eq!(zero.row.cost.estimated().map(|e| e.usd.get()), Some(4500));
    assert!(zero.row.approximate);
    let paid = events
        .iter()
        .find(|e| e.row.dedup_key.as_str().contains("resp_paid"))
        .unwrap();
    assert_eq!(
        paid.row.cost.reported().map(tokenmtr_core::UsdMicros::get),
        Some(500_000)
    );
    assert!(!paid.row.approximate);
}

#[test]
fn a_tool_result_in_a_later_scan_sets_the_error_flag_of_the_stored_call() {
    let mut machine = Machine::new();
    let path = machine.claude_dir().join("s1.jsonl");
    let tool_use = json!({"parentUuid": null, "isSidechain": false, "cwd": "/work/app",
        "sessionId": "s1", "version": "2.1.0", "type": "assistant",
        "message": {"id": "msg_t", "type": "message", "role": "assistant", "model": "claude-sonnet-4-5",
            "content": [{"type": "tool_use", "id": "toolu_a", "name": "Bash", "input": {}}],
            "stop_reason": null, "usage": {"input_tokens": 5, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0, "output_tokens": 5}},
        "requestId": "req_t", "uuid": "u-t", "timestamp": stamp(0)})
    .to_string();
    write(&path, &lines(&[tool_use]));
    machine.scan();
    let stored = |machine: &Machine| machine.store.tool_calls_after(Revision::ZERO, 100).unwrap();
    assert_eq!(stored(&machine)[0].row.call.is_error, None);

    let result = json!({"parentUuid": null, "isSidechain": false, "cwd": "/work/app",
        "sessionId": "s1", "version": "2.1.0", "type": "user",
        "message": {"role": "user", "content": [
            {"tool_use_id": "toolu_a", "type": "tool_result", "content": "boom", "is_error": true}]},
        "uuid": "u-r", "timestamp": stamp(1)})
    .to_string();
    append(&path, &lines(&[result]));
    let report = machine.scan();
    assert_eq!(claude_agent(&report).orphan_results_applied, 1);
    assert_eq!(stored(&machine)[0].row.call.is_error, Some(true));
}

#[test]
fn format_drift_merges_into_one_issue_per_file_and_closes_when_the_file_is_clean() {
    let mut machine = Machine::new();
    let drift = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/claude/projects/-home-demo-app/sess-drift.jsonl");
    let path = machine.claude_dir().join("sess-drift.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(&drift, &path).unwrap();
    let drift_issues = |machine: &Machine| -> Vec<_> {
        machine
            .ingestor
            .doctor(&machine.store, &[])
            .unwrap()
            .open_issues
            .into_iter()
            .filter(|i| i.kind == "format_drift")
            .collect()
    };
    let subjects = |machine: &Machine| -> Vec<String> {
        let mut subjects: Vec<_> = drift_issues(machine)
            .into_iter()
            .map(|i| i.subject)
            .collect();
        subjects.sort();
        subjects
    };
    // The unmapped line is the file's issue; the failed usage lines are over the threshold of
    // the whole scan (D42), whose issue has its own subject.
    let expected = vec![path.to_string(), super::SCAN_ISSUE_SUBJECT.to_owned()];
    machine.scan();
    assert_eq!(subjects(&machine), expected);
    machine.scan_with(&ScanRequest::full(ScanPriority::UserInitiated));
    machine.scan_with(&ScanRequest::full(ScanPriority::UserInitiated));
    assert_eq!(
        subjects(&machine),
        expected,
        "repeated scans merge into the same open issues"
    );

    write(&path, &lines(&[sonnet(0, "s1", "/work/app")]));
    machine.scan();
    assert_eq!(drift_issues(&machine).len(), 0);
}

#[test]
fn an_unreadable_file_is_an_issue_and_the_other_files_are_still_ingested() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut machine = Machine::new();
    let bad = session_file(&machine, "bad", 0..2, "/work/app");
    session_file(&machine, "good", 0..3, "/work/app");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&bad).is_ok() {
        return; // running as root: permissions do not apply
    }
    let report = machine.scan();
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(claude_agent(&report).files_failed, 1);
    assert_eq!(claude_agent(&report).events.inserted, 3);
    let issues = machine
        .ingestor
        .doctor(&machine.store, &[])
        .unwrap()
        .open_issues;
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.kind == "parse_error" && i.subject == bad.as_str())
            .count(),
        1
    );

    let retry = machine.scan();
    assert_eq!(
        claude_agent(&retry).events.inserted,
        2,
        "the failed file retries and succeeds"
    );
    let issues = machine
        .ingestor
        .doctor(&machine.store, &[])
        .unwrap()
        .open_issues;
    assert!(issues.iter().all(|i| i.kind != "parse_error"));
}

#[test]
fn a_user_initiated_scan_and_a_background_scan_store_the_same_rows() {
    let files = |machine: &Machine| {
        for (i, session) in ["a", "b", "c", "d"].iter().enumerate() {
            let count = 3 + u32::try_from(i).unwrap() * 4;
            session_file(machine, session, 0..count, "/work/app");
        }
    };
    let mut parallel = Machine::new();
    files(&parallel);
    parallel.scan_with(&ScanRequest::incremental(ScanPriority::UserInitiated));
    let mut sequential = Machine::new();
    files(&sequential);
    sequential.scan_with(&ScanRequest::incremental(ScanPriority::Background));
    assert_eq!(parallel.snapshot(), sequential.snapshot());
    assert_eq!(parallel.snapshot().len(), 3 + 7 + 11 + 15);
}

#[test]
fn a_scan_of_named_paths_parses_only_those() {
    let mut machine = Machine::new();
    let a = session_file(&machine, "a", 0..2, "/work/app");
    session_file(&machine, "b", 0..3, "/work/app");
    let stranger = machine.root.join("elsewhere.jsonl");
    write(&stranger, "{}\n");
    let missing = machine.claude_dir().join("gone.jsonl");
    let request =
        ScanRequest::incremental(ScanPriority::Background).for_paths(vec![a, stranger, missing]);
    let report = machine.scan_with(&request);
    assert_eq!(claude_agent(&report).files_discovered, 1);
    assert_eq!(machine.snapshot().len(), 2);
    assert!(claude_agent(&report).aggregates.is_none());
}

#[test]
fn a_scan_can_be_limited_to_one_agent() {
    let mut machine = Machine::new();
    session_file(&machine, "a", 0..2, "/work/app");
    let report = machine
        .scan_with(&ScanRequest::incremental(ScanPriority::UserInitiated).for_agent(AgentId::PI));
    assert_eq!(report.agents.len(), 1);
    assert_eq!(machine.snapshot().len(), 0);
}

#[test]
fn a_missing_cwd_raises_one_issue_however_often_it_is_seen() {
    let mut machine = Machine::new();
    session_file(&machine, "a", 0..3, "/definitely/not/here");
    machine.scan();
    machine.scan_with(&ScanRequest::full(ScanPriority::UserInitiated));
    let report = machine.ingestor.doctor(&machine.store, &[]).unwrap();
    let missing: Vec<_> = report
        .open_issues
        .iter()
        .filter(|i| i.kind == "missing_cwd")
        .collect();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].occurrences, 1);
}

#[test]
fn a_cwd_that_appears_later_is_retried_after_a_restart() {
    let mut machine = Machine::new();
    let cwd = machine.root.join("later");
    session_file(&machine, "a", 0..2, cwd.as_str());
    machine.scan();
    let open = |machine: &Machine| {
        let report = machine.ingestor.doctor(&machine.store, &[]).unwrap();
        report
            .open_issues
            .iter()
            .filter(|i| i.kind == "missing_cwd")
            .count()
    };
    assert_eq!(open(&machine), 1);

    fs::create_dir_all(&cwd).unwrap();
    machine.scan();
    assert_eq!(
        open(&machine),
        1,
        "retried once per process, not every scan"
    );
    machine.reconfigure("");
    machine.scan();
    assert_eq!(open(&machine), 0);
}

#[test]
fn a_cwd_under_a_protected_folder_is_not_read_and_raises_its_issue() {
    let mut machine = Machine::new();
    let cwd = machine.root.join("protected").join("app");
    fs::create_dir_all(cwd.join(".git")).unwrap();
    session_file(&machine, "a", 0..1, cwd.as_str());
    machine.scan();
    let report = machine.ingestor.doctor(&machine.store, &[]).unwrap();
    assert!(
        report
            .open_issues
            .iter()
            .any(|i| i.kind == "protected_folder")
    );
}

#[test]
fn an_abandoned_half_line_is_skipped_so_the_next_line_parses_alone() {
    let mut machine = Machine::new();
    let path = session_file(&machine, "s1", 0..2, "/work/app");
    let half = sonnet(2, "s1", "/work/app");
    append(&path, &half[..half.len() / 2]);
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3_600);
    fs::File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    let report = machine.scan();
    assert_eq!(claude_agent(&report).events.inserted, 2);
    assert_eq!(
        claude_agent(&report).files[0].to,
        fs::metadata(&path).unwrap().len(),
        "the crashed writer's tail is consumed"
    );

    append(&path, &lines(&[sonnet(3, "s1", "/work/app")]));
    let report = machine.scan();
    assert_eq!(claude_agent(&report).files[0].reason, ParseReason::Appended);
    assert_eq!(claude_agent(&report).events.inserted, 1);
    assert_eq!(machine.snapshot().len(), 3);
}

/// A pi fork copies its parent's entries with their ids and keys (M6): the copies keep the
/// parent's attribution, in whatever order the files are scanned.
#[test]
fn a_fork_never_takes_its_parents_events() {
    let message = |id: &str, response: &str| {
        json!({"type": "message", "id": id, "parentId": null,
            "timestamp": "2026-03-03T08:00:05.000Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "x"}],
                "api": "anthropic-messages", "provider": "anthropic",
                "model": "claude-sonnet-4-5", "responseId": response,
                "usage": {"input": 10, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 11, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                    "cacheWrite": 0, "total": 0.01}},
                "stopReason": "stop", "timestamp": 1_772_524_805_000_i64}})
        .to_string()
    };
    let header = |id: &str, parent: Option<&str>| {
        let mut header = json!({"type": "session", "version": 3, "id": id,
            "timestamp": "2026-03-03T08:00:00.000Z", "cwd": format!("/work/{id}")});
        if let Some(parent) = parent {
            header["parentSession"] = json!(parent);
        }
        header.to_string()
    };
    let sessions = |machine: &Machine| -> Vec<(String, String)> {
        machine
            .events()
            .into_iter()
            .map(|e| {
                let cwd = e.row.cwd.map(|cwd| cwd.to_string()).unwrap_or_default();
                (e.row.session.as_str().to_owned(), cwd)
            })
            .collect()
    };
    let expected = [
        ("parent".to_owned(), "/work/parent".to_owned()),
        ("fork".to_owned(), "/work/fork".to_owned()),
    ];
    for fork_first in [false, true] {
        for one_scan in [false, true] {
            let mut machine = Machine::new();
            let parent = machine
                .pi_dir()
                .join("2026-03-03T08-00-00-000Z_parent.jsonl");
            let fork = machine.pi_dir().join("2026-03-03T09-00-00-000Z_fork.jsonl");
            let parent_body = lines(&[header("parent", None), message("m1", "resp_a")]);
            let fork_body = lines(&[
                header("fork", Some(parent.as_str())),
                message("m1", "resp_a"),
                message("m2", "resp_b"),
            ]);
            let (first, second) = if fork_first {
                ((&fork, &fork_body), (&parent, &parent_body))
            } else {
                ((&parent, &parent_body), (&fork, &fork_body))
            };
            write(first.0, first.1);
            if !one_scan {
                machine.scan();
            }
            write(second.0, second.1);
            machine.scan();
            assert_eq!(
                sessions(&machine),
                expected,
                "fork first: {fork_first}, one scan: {one_scan}"
            );
        }
    }
}

#[test]
fn stats_cache_fills_only_the_days_before_the_first_transcript_event() {
    let mut machine = Machine::new();
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude/stats-cache.json");
    let config_dir = machine.root.join("claude");
    fs::create_dir_all(&config_dir).unwrap();
    fs::copy(&cache, config_dir.join("stats-cache.json")).unwrap();

    // No transcripts yet: every day of the cache is imported, once.
    let first = machine.scan();
    let aggregates = claude_agent(&first).aggregates.unwrap();
    assert_eq!(aggregates.written.inserted, 3);
    assert_eq!(aggregates.covered_by_transcripts, 0);
    let head = machine.head();
    let second = machine.scan();
    let again = claude_agent(&second).aggregates.unwrap();
    assert_eq!(
        (
            again.written.inserted,
            again.written.updated,
            again.written.unchanged
        ),
        (0, 0, 3)
    );
    assert_eq!(machine.head(), head, "a second import adds nothing");
    assert!(
        machine
            .events()
            .iter()
            .all(|e| e.row.approximate && e.row.at.to_string().contains("T12:00:00"))
    );

    // A transcript on 2025-09-02 covers that day (and later): only 2025-09-01 stays valid.
    let line = claude_line(0, "s1", "/work/app", "claude-sonnet-4-5", 10, 10)
        .replace("2026-03-07T01:00:00.000Z", "2025-09-02T10:00:00.000Z");
    write(&machine.claude_dir().join("s1.jsonl"), &lines(&[line]));
    let third = machine.scan();
    let covered = claude_agent(&third).aggregates.unwrap();
    assert_eq!(covered.found, 3);
    assert_eq!(
        covered.covered_by_transcripts, 1,
        "2025-09-02 is a transcript day"
    );
    assert_eq!(covered.written.inserted, 0);
}

#[test]
fn the_stats_cache_drift_issue_is_recorded_for_a_row_without_a_date() {
    let mut machine = Machine::new();
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude/stats-cache.json");
    let config_dir = machine.root.join("claude");
    fs::create_dir_all(&config_dir).unwrap();
    fs::copy(&cache, config_dir.join("stats-cache.json")).unwrap();
    machine.scan();
    let issues = machine
        .ingestor
        .doctor(&machine.store, &[])
        .unwrap()
        .open_issues;
    let drift: Vec<_> = issues.iter().filter(|i| i.kind == "format_drift").collect();
    assert_eq!(drift.len(), 1);
    assert!(drift[0].subject.ends_with("stats-cache.json"));
}

#[test]
fn doctor_reports_roots_checkpoints_retention_and_protected_paths() {
    let mut machine = Machine::new();
    session_file(&machine, "a", 0..2, "/work/app");
    session_file(
        &machine,
        "b",
        0..2,
        &format!("{}/protected/app", machine.root),
    );
    fs::write(
        machine.root.join("claude").join("settings.json"),
        r#"{"cleanupPeriodDays": 7}"#,
    )
    .unwrap();
    machine.scan();
    let sink = machine.root.join("protected").join("sink");
    let report = machine
        .ingestor
        .doctor(&machine.store, std::slice::from_ref(&sink))
        .unwrap();

    let claude = report
        .agents
        .iter()
        .find(|a| a.agent == AgentId::CLAUDE)
        .unwrap();
    assert_eq!(claude.checkpoints, 2);
    assert!(claude.last_run.is_some());
    let root = &claude.profiles[0].roots[0];
    assert!(root.exists);
    assert_eq!(root.files, 2);
    assert!(root.bytes > 0);
    let retention = claude.profiles[0].retention.unwrap();
    assert_eq!(
        (retention.cleanup_period_days, retention.configured),
        (7, true)
    );
    assert_eq!(retention.oldest_age_days, Some(0));
    assert_eq!(retention.days_until_oldest_expires, Some(7));

    let pi = report
        .agents
        .iter()
        .find(|a| a.agent == AgentId::PI)
        .unwrap();
    assert!(!pi.profiles[0].roots[0].exists);
    assert_eq!(
        report.protected_cwds,
        [machine.root.join("protected").join("app")]
    );
    assert_eq!(report.protected_sinks, [sink]);
    assert_eq!(report.unresolved_cwds, Vec::<Utf8PathBuf>::new());
}

#[test]
fn the_ingestor_knows_which_paths_are_its_own() {
    let machine = Machine::new();
    let inside = machine.claude_dir().join("s.jsonl");
    assert!(machine.ingestor.owns(&inside));
    assert!(
        !machine
            .ingestor
            .owns(&machine.claude_dir().join("notes.txt"))
    );
    assert!(!machine.ingestor.owns(&machine.root.join("s.jsonl")));
    let roots = machine.ingestor.roots();
    assert_eq!(roots.len(), 2);
}

#[test]
fn full_mode_reparses_every_file() {
    let mut machine = Machine::new();
    session_file(&machine, "a", 0..2, "/work/app");
    machine.scan();
    let report = machine.scan_with(&ScanRequest {
        agents: None,
        mode: ScanMode::Full,
        priority: ScanPriority::Background,
        paths: None,
    });
    assert_eq!(claude_agent(&report).files[0].reason, ParseReason::Forced);
}
