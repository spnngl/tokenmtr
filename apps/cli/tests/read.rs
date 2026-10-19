//! `status`, `recap`, `alerts`, `quota`, `fx`, `pricing` and `report` against a seeded store.

mod common;

use common::{CONFIG, Home, REPORT_MONTH, TestResult, seed};
use predicates::prelude::*;
use serde_json::Value;

fn seeded() -> Result<Home, Box<dyn std::error::Error>> {
    let home = Home::new()?;
    home.write_config(CONFIG)?;
    seed(&home)?;
    Ok(home)
}

fn json(home: &Home, args: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
    let output = home
        .tokenmtr()
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn status_reports_a_daemon_that_never_started() -> TestResult {
    let home = seeded()?;
    home.tokenmtr()
        .arg("status")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("never started").or(predicate::str::contains("never_started")),
        )
        .stdout(predicate::str::contains("claude"));
    Ok(())
}

#[test]
fn status_json_carries_the_schema_version_and_the_followed_agent() -> TestResult {
    let home = seeded()?;
    let status = json(&home, &["status", "--json"])?;
    assert_eq!(status["status_schema"], 1);
    assert_eq!(status["system"]["daemon"]["state"], "never_started");
    assert_eq!(status["agents"][0]["agent"], "claude");
    assert_eq!(status["agents"][0]["totals"]["api_value_micros"], 2_500_000);
    Ok(())
}

#[test]
fn status_creates_an_empty_store_when_there_is_none() -> TestResult {
    let home = Home::new()?;
    let status = json(&home, &["status", "--json"])?;
    assert_eq!(status["status_schema"], 1);
    Ok(())
}

#[test]
fn recap_json_is_one_agent_flattened() -> TestResult {
    let home = seeded()?;
    let recap = json(
        &home,
        &["recap", "--agent", "claude", "--period", "month", "--json"],
    )?;
    assert_eq!(recap["status_schema"], 1);
    assert_eq!(recap["agent"], "claude");
    assert_eq!(recap["period"], "month");
    Ok(())
}

#[test]
fn recap_prints_money_for_today() -> TestResult {
    let home = seeded()?;
    home.tokenmtr()
        .args(["recap", "--agent", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2.50 USD"));
    Ok(())
}

#[test]
fn alerts_list_the_fired_alert() -> TestResult {
    let home = seeded()?;
    home.tokenmtr()
        .arg("alerts")
        .assert()
        .success()
        .stdout(predicate::str::contains("80"));
    let alerts = json(&home, &["alerts", "--json"])?;
    assert_eq!(alerts.as_array().map(Vec::len), Some(1));
    Ok(())
}

#[test]
fn alerts_on_an_empty_store_say_so() -> TestResult {
    let home = Home::new()?;
    home.tokenmtr()
        .arg("alerts")
        .assert()
        .success()
        .stdout(predicate::str::contains("no alerts"));
    Ok(())
}

#[test]
fn quota_shows_the_window_and_the_percent() -> TestResult {
    let home = seeded()?;
    home.tokenmtr()
        .arg("quota")
        .assert()
        .success()
        .stdout(predicate::str::contains("42"));
    let quota = json(&home, &["quota", "--json"])?;
    assert_eq!(quota["meters"][0]["windows"][0]["used_percent"], 42.0);
    // The extra-usage spend window comes last, with its money.
    let extra = &quota["meters"][0]["windows"][1];
    assert_eq!(extra["window"], "extra_usage");
    assert_eq!(extra["resets_at"], serde_json::Value::Null);
    assert_eq!(extra["spend"]["spent_micros"], 12_345_000);
    assert_eq!(extra["spend"]["limit_micros"], 50_000_000);
    Ok(())
}

#[test]
fn quota_connect_needs_the_daemon() -> TestResult {
    let home = seeded()?;
    let expected = if cfg!(target_os = "macos") {
        "the daemon is not running"
    } else {
        // No Keychain elsewhere: the command stops before it looks for the daemon.
        "only macOS keeps the Claude credentials in a Keychain"
    };
    home.tokenmtr()
        .args(["quota", "connect"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(expected));
    Ok(())
}

#[test]
fn fx_shows_the_latest_rate_and_its_date() -> TestResult {
    let home = seeded()?;
    home.tokenmtr()
        .arg("fx")
        .assert()
        .success()
        .stdout(predicate::str::contains("0.9000"))
        .stdout(predicate::str::contains("2026-03-09"));
    Ok(())
}

#[test]
fn fx_prefers_a_fixed_rate_from_the_config() -> TestResult {
    let home = seeded()?;
    home.write_config(&format!(
        "[fx]\nfixed_eur_per_usd = 0.5\n{}",
        CONFIG.replace("[fx]\nenabled = false\n", "")
    ))?;
    home.tokenmtr()
        .arg("fx")
        .assert()
        .success()
        .stdout(predicate::str::contains("fixed rate"));
    Ok(())
}

#[test]
fn pricing_without_a_snapshot_lists_the_unpriced_state() -> TestResult {
    let home = seeded()?;
    home.tokenmtr().arg("pricing").assert().success();
    let pricing = json(&home, &["pricing", "--json"])?;
    assert!(pricing["snapshot"].is_null());
    Ok(())
}

#[test]
fn report_markdown_names_the_project_and_the_total() -> TestResult {
    let home = seeded()?;
    home.tokenmtr()
        .args(["report", "--month", REPORT_MONTH, "--format", "md"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme"))
        .stdout(predicate::str::contains("4.00"));
    Ok(())
}

#[test]
fn report_csv_goes_to_the_out_file() -> TestResult {
    let home = seeded()?;
    let out = home.root.join("report.csv");
    home.tokenmtr()
        .args([
            "report",
            "--month",
            REPORT_MONTH,
            "--format",
            "csv",
            "--out",
            out.as_str(),
        ])
        .assert()
        .success();
    assert!(std::fs::read_to_string(out)?.contains("acme"));
    Ok(())
}

#[test]
fn report_needs_a_range() -> TestResult {
    let home = seeded()?;
    home.tokenmtr().arg("report").assert().code(2);
    Ok(())
}

#[test]
fn a_usage_error_exits_with_2_and_a_failure_with_1() -> TestResult {
    let home = seeded()?;
    home.tokenmtr().arg("nonsense").assert().code(2);
    home.write_config("[quota\n")?;
    home.tokenmtr().args(["config", "check"]).assert().code(1);
    Ok(())
}

#[test]
fn a_newer_store_is_refused() -> TestResult {
    let home = seeded()?;
    let conn = rusqlite::Connection::open(home.store_path().as_std_path())?;
    conn.pragma_update(None, "user_version", tokenmtr_store::SCHEMA_VERSION + 1)?;
    drop(conn);
    home.tokenmtr()
        .arg("status")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("schema"));
    Ok(())
}
