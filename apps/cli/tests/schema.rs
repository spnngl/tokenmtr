//! `status --json` validates against `docs/status.schema.json`.

mod common;

use common::{CONFIG, Home, TestResult, seed};
use serde_json::{Value, json};

const SCHEMA: &str = include_str!("../../../docs/status.schema.json");

fn run_json(home: &Home, args: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
    let output = home
        .tokenmtr()
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn errors(instance: &Value) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let validator = jsonschema::validator_for(&serde_json::from_str(SCHEMA)?)?;
    Ok(validator
        .iter_errors(instance)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect())
}

fn seeded(extra: &str) -> Result<Home, Box<dyn std::error::Error>> {
    let home = Home::new()?;
    home.write_config(&format!(
        "{CONFIG}{extra}\n[[budgets]]\nid = \"month\"\nperiod = \"month\"\namount = 100\ncurrency = \"USD\"\nmeasure = \"paid\"\n"
    ))?;
    seed(&home)?;
    Ok(home)
}

#[test]
fn the_schema_is_valid_json_schema() -> TestResult {
    let schema: Value = serde_json::from_str(SCHEMA)?;
    assert!(jsonschema::meta::is_valid(&schema));
    Ok(())
}

#[test]
fn a_seeded_status_validates() -> TestResult {
    let home = seeded(
        "[[sinks]]\nid = \"csv\"\nkind = \"csv\"\ninterval = \"1h\"\ndatasets = [\"usage_events\"]\n",
    )?;
    let status = run_json(&home, &["status", "--json"])?;
    assert_eq!(errors(&status)?, Vec::<String>::new());
    assert_eq!(status["system"]["sinks"][0]["sink"], "csv");
    assert_eq!(status["agents"][0]["budgets"][0]["id"], "month");
    Ok(())
}

#[test]
fn an_empty_status_validates() -> TestResult {
    let home = Home::new()?;
    let status = run_json(&home, &["status", "--json"])?;
    assert_eq!(errors(&status)?, Vec::<String>::new());
    Ok(())
}

#[test]
fn a_recap_is_one_status_agent() -> TestResult {
    let home = seeded("")?;
    let recap = run_json(
        &home,
        &["recap", "--json", "--agent", "claude", "--period", "week"],
    )?;
    let wrapped = json!({
        "status_schema": recap["status_schema"],
        "generated_at": recap["generated_at"],
        "system": recap["system"],
        "agents": [recap],
    });
    assert_eq!(errors(&wrapped)?, Vec::<String>::new());
    Ok(())
}

#[test]
fn the_schema_rejects_a_broken_status() -> TestResult {
    let home = seeded("")?;
    let mut status = run_json(&home, &["status", "--json"])?;
    status["agents"][0]["totals"]["api_value_micros"] = json!("2.5");
    status["status_schema"] = json!(2);
    assert!(errors(&status)?.len() >= 2);
    Ok(())
}
