//! `tokenmtr sources`: the list, a direct import and forgetting a source.

mod common;

use common::{CONFIG, Home, TestResult, seed};
use predicates::prelude::*;
use serde_json::Value;

fn sqlite_source(id: &str, home: &Home, enabled: bool) -> String {
    format!(
        "{CONFIG}[[sources]]\nid = \"{id}\"\nkind = \"sqlite\"\nenabled = {enabled}\n\
         include = [\"{}/shared/*.db\"]\n",
        home.root
    )
}

#[test]
fn the_list_shows_the_implicit_superset_source() -> TestResult {
    let home = Home::new()?;
    home.write_config(CONFIG)?;
    seed(&home)?;
    home.tokenmtr()
        .arg("sources")
        .assert()
        .success()
        .stdout(predicate::str::contains("superset (superset): healthy"));
    Ok(())
}

#[test]
fn the_json_list_names_the_state_of_every_source() -> TestResult {
    let home = Home::new()?;
    home.write_config(&sqlite_source("laptop", &home, false))?;
    seed(&home)?;
    let output = home
        .tokenmtr()
        .args(["sources", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let sources: Value = serde_json::from_slice(&output)?;
    let laptop = sources
        .as_array()
        .and_then(|all| all.iter().find(|s| s["source"] == "laptop"))
        .ok_or("no laptop source")?;
    assert_eq!(laptop["kind"], "sqlite");
    assert_eq!(laptop["state"], "disabled");
    Ok(())
}

#[test]
fn an_import_of_a_source_that_is_not_enabled_fails() -> TestResult {
    let home = Home::new()?;
    home.write_config(&sqlite_source("laptop", &home, false))?;
    seed(&home)?;
    home.tokenmtr()
        .args(["sources", "import", "laptop"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no enabled sqlite source"));
    Ok(())
}

#[test]
fn an_enabled_source_with_no_files_imports_nothing() -> TestResult {
    let home = Home::new()?;
    home.write_config(&sqlite_source("laptop", &home, true))?;
    seed(&home)?;
    home.tokenmtr()
        .args(["sources", "import"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "laptop: 0 files read, 0 rows written",
        ));
    Ok(())
}

#[test]
fn forgetting_an_enabled_source_is_refused_and_a_disabled_one_is_accepted() -> TestResult {
    let home = Home::new()?;
    home.write_config(&sqlite_source("laptop", &home, true))?;
    seed(&home)?;
    home.tokenmtr()
        .args(["sources", "forget", "laptop"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("still enabled"));

    home.write_config(&sqlite_source("laptop", &home, false))?;
    home.tokenmtr()
        .args(["sources", "forget", "laptop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("forgot 0 events and 0 tool calls"));
    Ok(())
}
