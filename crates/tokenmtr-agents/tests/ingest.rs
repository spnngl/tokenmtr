//! The ingestor through its public API, on the committed fixtures (the scan completes,
//! and a second scan parses nothing).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use camino::Utf8PathBuf;
use tokenmtr_agents::ingest::{IngestError, IngestSettings, Ingestor, ScanPriority, ScanRequest};
use tokenmtr_config::{ConfigFormat, load_str};
use tokenmtr_core::AgentId;
use tokenmtr_platform::Paths;
use tokenmtr_store::{Dataset, RepriceScope, Store, StoreRead as _};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixtures() -> Result<Utf8PathBuf, Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .canonicalize()?;
    Utf8PathBuf::from_path_buf(path).map_err(|p| format!("not utf-8: {}", p.display()).into())
}

/// Settings built by the real config loader, with both agents pointed at the fixtures.
fn settings(home: &Utf8PathBuf) -> Result<IngestSettings, Box<dyn std::error::Error>> {
    let fixtures = fixtures()?;
    let toml = format!(
        "[[agents.claude.profiles]]\nid = \"default\"\nconfig_dir = \"{fixtures}/claude\"\n\
         [[agents.pi.profiles]]\nid = \"default\"\nroots = [\"{fixtures}/pi/sessions\"]\n"
    );
    let format = ConfigFormat::from_path(camino::Utf8Path::new("config.toml")).ok_or("toml")?;
    let config = load_str(&toml, format, &|_: &str| None::<String>)?;
    let home = home.clone();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.to_string()))?;
    Ok(IngestSettings::from_config(&config, &paths, &|_: &str| {
        None::<String>
    }))
}

#[test]
fn scanning_the_fixtures_completes_and_a_second_scan_reads_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let home = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?).map_err(|_| "utf-8")?;
    let db = home.join("store.db");
    let mut store = Store::open(&db)?;
    let mut ingestor = Ingestor::open(settings(&home)?, &db)?;

    let first = ingestor.scan(
        &mut store,
        &ScanRequest::incremental(ScanPriority::UserInitiated),
    )?;
    let claude = first.agent(AgentId::CLAUDE).ok_or("claude not scanned")?;
    let pi = first.agent(AgentId::PI).ok_or("pi not scanned")?;
    assert!(
        claude.files_parsed >= 10,
        "claude files: {}",
        claude.files_parsed
    );
    assert!(pi.files_parsed >= 8, "pi files: {}", pi.files_parsed);
    assert_eq!(claude.files_failed + pi.files_failed, 0);
    assert!(first.events_changed() > 0);
    assert!(claude.megabytes_per_second().is_some());
    let head = store.head_revision(Dataset::UsageEvents)?;

    let second = ingestor.scan(
        &mut store,
        &ScanRequest::incremental(ScanPriority::Background),
    )?;
    assert_eq!(second.bytes_parsed(), 0);
    assert_eq!(store.head_revision(Dataset::UsageEvents)?, head);

    let report = ingestor.doctor(&store, &[])?;
    assert_eq!(report.agents.len(), 2);
    assert!(
        !report.open_issues.is_empty(),
        "the drift fixtures raise issues"
    );
    Ok(())
}

#[test]
fn a_raised_stop_flag_ends_scans_and_reprices_without_writing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let home = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?).map_err(|_| "utf-8")?;
    let db = home.join("store.db");
    let mut store = Store::open(&db)?;
    let stop = Arc::new(AtomicBool::new(true));
    let mut ingestor = Ingestor::open(settings(&home)?, &db)?.with_stop(Arc::clone(&stop));
    let head = store.head_revision(Dataset::UsageEvents)?;
    for priority in [ScanPriority::UserInitiated, ScanPriority::Background] {
        let stopped = ingestor.scan(&mut store, &ScanRequest::incremental(priority));
        assert!(matches!(stopped, Err(IngestError::Stopped)), "{stopped:?}");
    }
    let stopped = ingestor.reprice(&mut store, RepriceScope::All);
    assert!(matches!(stopped, Err(IngestError::Stopped)), "{stopped:?}");
    assert_eq!(store.head_revision(Dataset::UsageEvents)?, head);

    stop.store(false, Ordering::Relaxed);
    let report = ingestor.scan(
        &mut store,
        &ScanRequest::incremental(ScanPriority::UserInitiated),
    )?;
    assert!(report.events_changed() > 0, "a lowered flag scans again");
    Ok(())
}

#[test]
fn settings_follow_the_config_profiles() -> TestResult {
    let dir = tempfile::tempdir()?;
    let home = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?).map_err(|_| "utf-8")?;
    let settings = settings(&home)?;
    let fixtures = fixtures()?;
    let ingestor_roots: Vec<Utf8PathBuf> = {
        let db = home.join("store.db");
        let _store = Store::open(&db)?;
        Ingestor::open(settings, &db)?.roots()
    };
    assert_eq!(
        ingestor_roots,
        [
            fixtures.join("claude/projects"),
            fixtures.join("pi/sessions")
        ]
    );
    Ok(())
}
