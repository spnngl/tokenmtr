//! The real `tokenmtr` binary on a sandboxed `HOME` against mock servers.
//!
//! The daemon ingests the fixture agent files into both a CSV and a SQLite sink, polls the mocked
//! quota, FX and pricing endpoints once, serves the CLI, writes a backup and stops on SIGTERM in
//! under two seconds. The fake OAuth token shows up nowhere but its credentials file.

mod common;

use std::collections::BTreeSet;
use std::error::Error;
use std::io::Write as _;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use common::{
    ECB_HISTORY_PATH, LITELLM_PATH, PROFILE_PATH, Sandbox, TestResult, USAGE_PATH,
    assert_no_secret, eventually, mock_servers,
};
use serde_json::Value;
use tokenmtr_agents::ingest::{IngestSettings, Ingestor, ScanPriority, ScanRequest};
use tokenmtr_config::{ConfigFormat, load_str};
use tokenmtr_store::{Revision, Store, StoreRead as _};

const STATUS_SCHEMA: &str = include_str!("../../../docs/status.schema.json");
const WAIT: Duration = Duration::from_secs(30);

/// How many events the fixtures hold, by an in-process scan into a scratch store that is
/// independent of the daemon under test.
fn reference_event_count(sandbox: &Sandbox) -> Result<usize, Box<dyn Error>> {
    let scratch = tempfile::tempdir()?;
    let db = Utf8PathBuf::from_path_buf(scratch.path().join("reference.db"))
        .map_err(|p| format!("not utf-8: {}", p.display()))?;
    let claude = sandbox.claude_dir().display().to_string();
    let pi = sandbox.root.join("pi").to_string();
    let env = move |key: &str| match key {
        "CLAUDE_CONFIG_DIR" => Some(claude.clone()),
        "PI_CODING_AGENT_DIR" => Some(pi.clone()),
        _ => None,
    };
    let format = ConfigFormat::from_path(Utf8Path::new("config.toml")).ok_or("toml")?;
    let config = load_str("", format, &env)?;
    let settings = IngestSettings::from_config(&config, &sandbox.paths, &env);
    let mut store = Store::open(&db)?;
    Ingestor::open(settings, &db)?.scan(
        &mut store,
        &ScanRequest::incremental(ScanPriority::UserInitiated),
    )?;
    Ok(store.usage_events_after(Revision::ZERO, 100_000)?.len())
}

/// Waits until both sinks hold exactly `count` events, the same ones.
fn wait_for_sinks(sandbox: &Sandbox, count: usize) -> Result<BTreeSet<String>, Box<dyn Error>> {
    eventually(WAIT, &format!("{count} events in both sinks"), || {
        let csv = sandbox.csv_event_ids()?;
        let sqlite = sandbox.sqlite_event_ids()?;
        Ok((csv.len() == count && csv == sqlite).then_some(csv))
    })
}

/// A copy of the one-line fixture session with fresh message, request and record ids.
fn appended_line() -> Result<String, Box<dyn Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/claude/projects/-home-demo-app/sess-no-cache-creation.jsonl");
    let line = std::fs::read_to_string(path)?;
    Ok(line
        .replace("msg_n1", "msg_e2e")
        .replace("req_n1", "req_e2e")
        .replace("sess-no-cache-creation-1", "sess-no-cache-creation-e2e"))
}

fn append(sandbox: &Sandbox, text: &str) -> TestResult {
    let path = sandbox
        .claude_projects()
        .join("-home-demo-app/sess-no-cache-creation.jsonl");
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn window(quota: &Value, name: &str) -> Option<f64> {
    quota["meters"][0]["windows"]
        .as_array()?
        .iter()
        .find(|w| w["window"] == name)?["used_percent"]
        .as_f64()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_end_to_end() -> TestResult {
    let server = mock_servers(true).await?;
    let base = server.uri();
    let sandbox = Sandbox::new()?;
    sandbox.write_config(&sandbox.config(true))?;
    let expected = reference_event_count(&sandbox)?;
    assert!(expected > 10, "the fixtures hold {expected} events");

    let daemon = sandbox.start_daemon(&base)?;

    // Ingestion: every fixture event reaches both sinks, once.
    wait_for_sinks(&sandbox, expected)?;

    // A line appended to an agent file propagates to both sinks.
    append(&sandbox, &appended_line()?)?;
    wait_for_sinks(&sandbox, expected + 1)?;

    // `status --json` honours the published schema and sees the running daemon.
    let status = sandbox.cli_json(&base, &["status", "--json"])?;
    let schema = serde_json::from_str(STATUS_SCHEMA)?;
    let validator = jsonschema::validator_for(&schema)?;
    let errors: Vec<String> = validator
        .iter_errors(&status)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "status --json: {errors:#?}");
    assert_eq!(status["system"]["daemon"]["state"], "running", "{status}");

    // `quota --json` shows the mocked windows.
    let quota = eventually(WAIT, "the quota poll", || {
        let quota = sandbox.cli_json(&base, &["quota", "--json"])?;
        Ok((quota["meters"][0]["status"] == "ok").then_some(quota))
    })?;
    assert_eq!(window(&quota, "five_hour"), Some(37.0), "{quota}");
    assert_eq!(window(&quota, "seven_day"), Some(26.5), "{quota}");
    assert_eq!(window(&quota, "seven_day_sonnet"), Some(5.0), "{quota}");

    // The FX history and the price list arrived through the mock.
    let fx = eventually(WAIT, "the FX rates", || {
        let fx = sandbox.cli_json(&base, &["fx", "--json"])?;
        Ok((!fx["latest"].is_null()).then_some(fx))
    })?;
    assert!(fx["fixed_eur_per_usd_scaled"].is_null(), "{fx}");
    let pricing = eventually(WAIT, "the price list", || {
        let pricing = sandbox.cli_json(&base, &["pricing", "--json"])?;
        // The embedded snapshot has no source; a fetched one names where it came from.
        Ok((!pricing["snapshot"]["source"].is_null()).then_some(pricing))
    })?;
    assert!(pricing["snapshot"]["version"].is_string(), "{pricing}");

    // The control channel answers; its answers are part of the credential-leak scan.
    let answers = sandbox.control(&[r#"{"type":"status"}"#, r#"{"type":"issues"}"#])?;
    assert!(answers[0].contains("welcome"), "{answers:?}");

    // A backup through the daemon; both the file and its decompressed content are scanned.
    let backup = sandbox.cli_json(&base, &["backup", "now", "--json"])?;
    let backup_path = Utf8PathBuf::from(backup["path"].as_str().ok_or("backup path")?);
    let restored = sandbox.root.join("restored.db");
    tokenmtr_store::restore(&backup_path, &restored)?;
    let extra = vec![
        ("<backup file>".to_owned(), std::fs::read(&backup_path)?),
        ("<restored backup>".to_owned(), std::fs::read(&restored)?),
    ];

    // SIGTERM: a clean, fast exit that removes the socket.
    let (code, elapsed) = daemon.terminate()?;
    assert_eq!(code, Some(0), "exit code");
    assert!(
        elapsed < Duration::from_secs(2),
        "stopped in {elapsed:?}, limit 2s"
    );
    assert!(!sandbox.socket().exists(), "the socket is removed");

    // Every endpoint was hit exactly once (the mocks carry `expect(1)`); nothing else was hit.
    let requests = server.received_requests().await.ok_or("not recording")?;
    let paths: BTreeSet<&str> = requests.iter().map(|r| r.url.path()).collect();
    assert_eq!(
        paths,
        BTreeSet::from([USAGE_PATH, PROFILE_PATH, ECB_HISTORY_PATH, LITELLM_PATH]),
        "{requests:#?}"
    );
    assert_eq!(requests.len(), 4, "{requests:#?}");
    server.verify().await;

    assert_no_secret(&sandbox, &extra)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_every_networked_task_off_the_server_receives_nothing() -> TestResult {
    let server = mock_servers(false).await?;
    let base = server.uri();
    let sandbox = Sandbox::new()?;
    sandbox.write_config(&sandbox.config(false))?;
    let expected = reference_event_count(&sandbox)?;

    let daemon = sandbox.start_daemon(&base)?;
    // The networked jobs run at startup, so once ingestion is done they would have fired.
    wait_for_sinks(&sandbox, expected)?;
    let quota = sandbox.cli_json(&base, &["quota", "--json"])?;
    assert_eq!(quota["meters"], serde_json::json!([]), "{quota}");
    sandbox.cli_json(&base, &["status", "--json"])?;

    let (code, _) = daemon.terminate()?;
    assert_eq!(code, Some(0), "exit code");
    let requests = server.received_requests().await.ok_or("not recording")?;
    assert!(requests.is_empty(), "{requests:#?}");

    assert_no_secret(&sandbox, &[])
}

#[test]
fn the_endpoint_override_refuses_anything_but_loopback_http() -> TestResult {
    let sandbox = Sandbox::new()?;
    sandbox.write_config("")?;
    for base in [
        "https://api.anthropic.com",
        "http://example.com",
        "not a url",
    ] {
        let output = sandbox.cli(base, &["run"])?;
        assert_eq!(output.status.code(), Some(1), "{base}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("TOKENMTR_TEST_ENDPOINT_BASE"), "{stderr}");
    }
    Ok(())
}
