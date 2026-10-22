//! `[[sources]]` of kind `sqlite`: another machine's export reaches the store on start, on a file
//! event, and never while the source is disabled.

mod common;

use camino::{Utf8Path, Utf8PathBuf};
use common::{Env, TestResult, WAIT, eventually, fast_timing};
use tokenmtr_core::{
    AgentId, DedupKey, Estimate, EventCost, EventId, EventKind, ModelId, PricingVersion, ProfileId,
    ProviderId, SessionId, SinkId, SourceId, TokenCounts, UsageEvent, UsdMicros,
};
use tokenmtr_export::{Sink, SqliteSink, SqliteTarget, fetch_batch};
use tokenmtr_store::{Dataset, Revision, Store, StoreRead as _};

/// A usage event of `machine` for `session`.
fn usage(store: &Store, session: &str) -> Result<UsageEvent, Box<dyn std::error::Error>> {
    let dedup_key = DedupKey::try_from(format!("{session}|2026-05-01T10:00:00Z"))?;
    Ok(UsageEvent {
        id: EventId::new(&store.machine_id(), AgentId::CLAUDE, &dedup_key),
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key,
        at: "2026-05-01T10:00:00Z".parse()?,
        session: SessionId::try_from(session)?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        project: None,
        client: None,
        source_file: None,
        approximate: false,
        provider: Some(ProviderId::try_from("anthropic")?),
        model: ModelId::try_from("claude-sonnet-4")?,
        kind: EventKind::Message,
        tokens: TokenCounts {
            input_uncached: 100,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            output: 500,
            reasoning: 0,
        },
        cost: EventCost::new(
            None,
            Some(Estimate {
                usd: UsdMicros::new(1_000),
                pricing_version: PricingVersion::of_body(b"snapshot"),
            }),
        ),
        service_tier: None,
    })
}

/// Exports one event of a fresh machine to `path`, written beside it and renamed over so the
/// watcher sees one event.
fn export_to(path: &Utf8Path, session: &str) -> TestResult {
    let scratch = tempfile::tempdir()?;
    let scratch_path = Utf8PathBuf::from_path_buf(scratch.path().to_path_buf())
        .map_err(|p| format!("not utf-8: {}", p.display()))?;
    let mut store = Store::open(&scratch_path.join("usage.db"))?;
    store.upsert_events(&[usage(&store, session)?])?;
    let batch = fetch_batch(&store, Dataset::UsageEvents, Revision::ZERO, 100)?;
    let staged = scratch_path.join("export.db");
    SqliteSink::new(
        SinkId::try_from("db")?,
        SqliteTarget {
            path: staged.clone(),
        },
    )
    .write(&batch)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, std::fs::read(&staged)?)?;
    Ok(())
}

fn source_config(id: &str, dir: &Utf8Path, enabled: bool) -> String {
    format!(
        "[[sources]]\nid = \"{id}\"\nkind = \"sqlite\"\nenabled = {enabled}\n\
         include = [\"{dir}/*.db\"]\n"
    )
}

fn imported(env: &Env, id: &str) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(env
        .reader()?
        .imported_rows(&SourceId::try_from(id)?)?
        .events)
}

#[tokio::test]
async fn a_source_is_imported_on_start_and_again_when_a_file_arrives() -> TestResult {
    let env = Env::new()?;
    let dir = env.home.join("shared");
    export_to(&dir.join("laptop.db"), "s1")?;
    env.write_config(&source_config("laptop", &dir, true))?;
    let daemon = env.start(fast_timing()).await?;

    assert!(
        eventually(WAIT, || imported(&env, "laptop").is_ok_and(|n| n == 1)).await,
        "the existing file is imported at start"
    );
    // The interval (30 s at least) is far away: only the file event can explain this.
    export_to(&dir.join("desktop.db"), "s2")?;
    assert!(
        eventually(WAIT, || imported(&env, "laptop").is_ok_and(|n| n == 2)).await,
        "a new file is imported on its event"
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_disabled_source_never_opens_its_files() -> TestResult {
    let env = Env::new()?;
    let off = env.home.join("off");
    let on = env.home.join("on");
    export_to(&off.join("a.db"), "s1")?;
    export_to(&on.join("b.db"), "s2")?;
    env.write_config(&format!(
        "{}{}",
        source_config("off", &off, false),
        source_config("on", &on, true)
    ))?;
    let daemon = env.start(fast_timing()).await?;

    assert!(
        eventually(WAIT, || imported(&env, "on").is_ok_and(|n| n == 1)).await,
        "the enabled source is imported"
    );
    let reader = env.reader()?;
    let off_id = SourceId::try_from("off")?;
    assert_eq!(imported(&env, "off")?, 0);
    assert_eq!(reader.import_cursors(&off_id)?.len(), 0);
    assert_eq!(reader.import_runs(&off_id, 10)?.len(), 0);
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_reload_enables_a_source_and_disabling_it_stops_the_imports() -> TestResult {
    let env = Env::new()?;
    let dir = env.home.join("shared");
    export_to(&dir.join("a.db"), "s1")?;
    env.write_config(&source_config("laptop", &dir, false))?;
    let daemon = env.start(fast_timing()).await?;
    let mut client = env.client().await?;

    env.write_config(&source_config("laptop", &dir, true))?;
    client.reload().await?;
    assert!(
        eventually(WAIT, || imported(&env, "laptop").is_ok_and(|n| n == 1)).await,
        "an enabled source is imported after a reload"
    );

    env.write_config(&source_config("laptop", &dir, false))?;
    client.reload().await?;
    export_to(&dir.join("b.db"), "s2")?;
    // Long enough for the file event and its debounce to have run an import.
    client.scan_now(None).await?;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(
        imported(&env, "laptop")?,
        1,
        "a disabled source stays as it was"
    );
    daemon.shutdown().await?;
    Ok(())
}
