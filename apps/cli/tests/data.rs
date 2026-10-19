//! Commands that change data: `scan`, `reprice`, `backup`, `restore`, `erase`, `config`,
//! `doctor`.

mod common;

use std::sync::Arc;

use camino::Utf8PathBuf;
use common::{CONFIG, Home, TestResult, seed};
use predicates::prelude::*;
use secrecy::SecretString;
use serde_json::Value;
use tokenmtr_daemon::{Daemon, Settings};
use tokenmtr_platform::{SecretError, SecretStore};
use tokenmtr_store::{Revision, StoreRead as _, StoreReader};

struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _service: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
}

const CLAUDE_LINE: &str = "{\"parentUuid\":null,\"isSidechain\":false,\"userType\":\"external\",\"cwd\":\"/tmp/app\",\"sessionId\":\"s1\",\"version\":\"2.1.0\",\"gitBranch\":\"main\",\"type\":\"assistant\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":20,\"cache_read_input_tokens\":30,\"output_tokens\":40}},\"requestId\":\"req_1\",\"uuid\":\"u1\",\"timestamp\":\"2026-03-05T11:00:01.000Z\"}\n";

/// A config that follows the Claude profile under the temp home.
fn config_with_claude(home: &Home) -> String {
    format!(
        "{CONFIG}[[agents.claude.profiles]]\nid = \"default\"\nconfig_dir = \"{}/claude\"\n",
        home.root
    )
}

fn seeded() -> Result<Home, Box<dyn std::error::Error>> {
    let home = Home::new()?;
    home.write_config(&config_with_claude(&home))?;
    seed(&home)?;
    Ok(home)
}

fn backup_now(home: &Home) -> Result<Utf8PathBuf, Box<dyn std::error::Error>> {
    home.tokenmtr().args(["backup", "now"]).assert().success();
    let dir = home.paths.data_dir().join("backups");
    let file = std::fs::read_dir(dir)?
        .next()
        .ok_or("no backup written")??
        .path();
    Ok(Utf8PathBuf::from_path_buf(file).map_err(|p| format!("{}", p.display()))?)
}

#[test]
fn scan_ingests_the_agents_files_directly() -> TestResult {
    let home = seeded()?;
    let logs = home.root.join("claude/projects/-tmp-app");
    std::fs::create_dir_all(&logs)?;
    std::fs::write(logs.join("s1.jsonl"), CLAUDE_LINE)?;
    home.tokenmtr()
        .args(["scan", "--agent", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude"));
    let reader = StoreReader::open(&home.store_path())?;
    // Two seeded events, the seeded extra-usage day and the scanned one.
    assert_eq!(reader.usage_events_after(Revision::ZERO, 100)?.len(), 4);
    Ok(())
}

fn repriced(home: &Home, args: &[&str]) -> Result<u64, Box<dyn std::error::Error>> {
    let output = home
        .tokenmtr()
        .args(["reprice", "--json"])
        .args(args)
        .output()?;
    assert!(output.status.success(), "{output:?}");
    let json: Value = serde_json::from_slice(&output.stdout)?;
    Ok(json["repriced"].as_u64().ok_or("no `repriced` count")?)
}

#[test]
fn reprice_estimates_once_then_has_nothing_left() -> TestResult {
    let home = seeded()?;
    repriced(&home, &[])?;
    assert_eq!(
        repriced(&home, &[])?,
        0,
        "every priceable event is estimated"
    );
    assert_eq!(
        repriced(&home, &["--all"])?,
        0,
        "estimates follow the active snapshot"
    );
    home.tokenmtr()
        .arg("reprice")
        .assert()
        .success()
        .stdout(predicate::str::contains("0 events repriced"));
    Ok(())
}

#[test]
fn backup_now_then_list_then_restore() -> TestResult {
    let home = seeded()?;
    let file = backup_now(&home)?;
    assert!(file.as_str().ends_with(".db.zst"));
    home.tokenmtr()
        .args(["backup", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(file.as_str()));
    home.tokenmtr()
        .args(["restore", file.as_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("restored"));
    // The restored store still answers.
    home.tokenmtr()
        .args(["status", "--json"])
        .assert()
        .success();
    Ok(())
}

#[test]
fn restore_rejects_a_file_that_is_no_backup() -> TestResult {
    let home = seeded()?;
    let bogus = home.root.join("bogus.db.zst");
    std::fs::write(&bogus, b"not zstd")?;
    home.tokenmtr()
        .args(["restore", bogus.as_str()])
        .assert()
        .code(1);
    home.tokenmtr()
        .args(["status", "--json"])
        .assert()
        .success();
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn full_scan_refuses_while_the_daemon_runs_and_restore_stops_it() -> TestResult {
    let home = seeded()?;
    let file = backup_now(&home)?;
    let daemon = Daemon::start(Settings::new(
        home.paths.clone(),
        home.config_path(),
        home.transport(),
        Arc::new(NoSecrets),
    ))
    .await?;
    // What `tokenmtr run` does: stop when a client asks for it.
    let stopper = tokio::spawn(async move {
        daemon.stop_requested().await;
        daemon.shutdown().await
    });

    let run = |args: Vec<String>| {
        let mut command = home.tokenmtr();
        tokio::task::spawn_blocking(move || command.args(args).assert())
    };
    let full_scan = run(vec!["scan".into(), "--full".into()]).await?;
    full_scan.code(1);
    let backup = run(vec!["backup".into(), "now".into()]).await?;
    backup
        .success()
        .stdout(predicate::str::contains("backup written"));
    let doctor = run(vec!["doctor".into(), "--json".into()]).await?;
    let output = doctor.success().get_output().clone();
    let doctor: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(doctor["daemon_running"], true);

    // `restore` asks the daemon to stop, then restores under the daemon lock.
    let restore = run(vec!["restore".into(), file.to_string()]).await?;
    restore
        .success()
        .stdout(predicate::str::contains("restored"));
    stopper.await??;
    assert!(!home.transport().socket_path().exists());
    assert!(home.store_path().exists());

    let erase = run(vec!["erase".into(), "--yes".into()]).await?;
    erase.success().stdout(predicate::str::contains("deleted"));
    assert!(!home.store_path().exists());
    Ok(())
}

#[test]
fn erase_needs_confirmation_and_keeps_outputs_and_agent_files() -> TestResult {
    let home = seeded()?;
    backup_now(&home)?;
    let logs = home.paths.logs_dir().join("daemon.log");
    let output = home.paths.exports_dir().join("usage.csv");
    let transcript = home.root.join("claude/projects/-tmp-app/s1.jsonl");
    for path in [&logs, &output, &transcript] {
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(path, "x")?;
    }

    home.tokenmtr()
        .arg("erase")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("--yes"));
    assert!(home.store_path().exists());

    home.tokenmtr()
        .args(["erase", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deleted"));
    assert!(!home.store_path().exists());
    let backups =
        std::fs::read_dir(home.paths.data_dir().join("backups")).map_or(0, Iterator::count);
    assert_eq!(backups, 0);
    assert!(!logs.exists());
    assert!(output.exists(), "sink outputs stay");
    assert!(transcript.exists(), "agent files stay");
    assert!(home.config_path().exists(), "the config stays by default");

    home.tokenmtr()
        .args(["erase", "--yes", "--delete-config"])
        .assert()
        .success();
    assert!(!home.config_path().exists());
    assert!(output.exists());
    Ok(())
}

#[test]
fn config_edits_keep_comments_and_validate() -> TestResult {
    let home = Home::new()?;
    home.write_config("# my settings\n[display]\ncurrency = \"EUR\" # euros\n")?;
    home.tokenmtr()
        .args([
            "config",
            "add-subscription",
            "--id",
            "max",
            "--plan",
            "max5x",
            "--start",
            "2026-01-01",
        ])
        .assert()
        .success();
    home.tokenmtr()
        .args([
            "config",
            "add-rule",
            "--match",
            "/tmp/acme/**",
            "--project",
            "acme",
            "--client",
            "Acme",
        ])
        .assert()
        .success();
    let text = std::fs::read_to_string(home.config_path())?;
    assert!(text.contains("# my settings"), "{text}");
    assert!(text.contains("# euros"), "{text}");
    assert!(text.contains("max"), "{text}");
    assert!(text.contains("/tmp/acme/**"), "{text}");
    home.tokenmtr().args(["config", "check"]).assert().success();
    home.tokenmtr()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(home.config_path().as_str()));
    Ok(())
}

#[test]
fn a_duplicate_subscription_is_refused_and_the_file_is_unchanged() -> TestResult {
    let home = Home::new()?;
    home.write_config("")?;
    let add = || {
        home.tokenmtr()
            .args([
                "config",
                "add-subscription",
                "--id",
                "max",
                "--plan",
                "max5x",
                "--start",
                "2026-01-01",
            ])
            .assert()
    };
    add().success();
    let before = std::fs::read_to_string(home.config_path())?;
    add().code(1);
    assert_eq!(std::fs::read_to_string(home.config_path())?, before);
    Ok(())
}

#[test]
fn doctor_reports_a_healthy_setup_as_json() -> TestResult {
    let home = seeded()?;
    let output = home
        .tokenmtr()
        .args(["doctor", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let doctor: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(doctor["config_error"], Value::Null);
    assert_eq!(doctor["daemon_running"], false);
    assert_eq!(doctor["store_schema"], tokenmtr_store::SCHEMA_VERSION);
    home.tokenmtr()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("not running"));
    Ok(())
}

#[test]
fn doctor_reports_an_invalid_config_then_fails() -> TestResult {
    let home = Home::new()?;
    home.write_config("[quota\n")?;
    let output = home
        .tokenmtr()
        .args(["doctor", "--json"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let doctor: Value = serde_json::from_slice(&output.stdout)?;
    assert!(doctor["config_error"].is_string());
    Ok(())
}
