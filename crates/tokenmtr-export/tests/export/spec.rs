//! Resolving config entries into sink specs.

use std::collections::BTreeMap;

use camino::Utf8Path;
use tokenmtr_config as config;
use tokenmtr_core::SinkId;
use tokenmtr_export::{SinkSpec, Target};
use tokenmtr_store::Dataset;

use crate::fixtures::TestResult;

fn entry(target: config::SinkTarget) -> TestResult<config::SinkConfig> {
    Ok(config::SinkConfig {
        id: SinkId::try_from("s")?,
        interval: jiff::SignedDuration::from_mins(5),
        datasets: vec![config::Dataset::FxRates, config::Dataset::UsageEvents],
        redact: BTreeMap::from([(config::RedactColumn::Cwd, config::RedactMode::Hash)]),
        target,
    })
}

#[test]
fn unset_locations_fall_back_to_the_exports_dir() -> TestResult {
    let exports = Utf8Path::new("/data/exports");
    let file = config::SinkTarget::File(config::FileSink {
        format: config::FileFormat::Jsonl,
        dir: None,
        mode: config::FileMode::Snapshot,
        partition: config::Partition::Day,
    });
    let spec = SinkSpec::from_config(&entry(file)?, exports);
    assert_eq!(spec.datasets, [Dataset::FxRates, Dataset::UsageEvents]);
    assert_eq!(spec.redact.len(), 1);
    let Target::File(target) = spec.target else {
        return Err("expected a file target".into());
    };
    assert_eq!(target.dir, exports);
    assert_eq!(target.mode, config::FileMode::Snapshot);

    let sqlite = config::SinkTarget::Sqlite(config::SqliteSink { path: None });
    let Target::Sqlite(target) = SinkSpec::from_config(&entry(sqlite)?, exports).target else {
        return Err("expected a sqlite target".into());
    };
    assert_eq!(target.path, "/data/exports/export.db");
    Ok(())
}

#[test]
fn explicit_locations_win() -> TestResult {
    let sqlite = config::SinkTarget::Sqlite(config::SqliteSink {
        path: Some("/elsewhere/x.db".into()),
    });
    let Target::Sqlite(target) =
        SinkSpec::from_config(&entry(sqlite)?, Utf8Path::new("/data/exports")).target
    else {
        return Err("expected a sqlite target".into());
    };
    assert_eq!(target.path, "/elsewhere/x.db");
    Ok(())
}
