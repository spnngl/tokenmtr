//! The daemon in-process on temp dirs: ingest, sinks, control requests, push events, alerts,
//! reload, shutdown (Phase 8 "Done when").

mod common;

use std::time::{Duration, Instant};

use common::{Env, TestResult, append, claude_line, eventually, fast_timing, seed_logs};
use tokenmtr_control::{Dataset, Event, Task};
use tokenmtr_core::AlertKind;
use tokenmtr_daemon::{Daemon, DaemonError};
use tokenmtr_store::StoreRead as _;

const FIVE_SECONDS: Duration = Duration::from_secs(5);

fn csv_sink(env: &Env, interval: &str) -> String {
    format!(
        "[[sinks]]\nid = \"csv\"\nkind = \"csv\"\ndir = \"{}/out\"\npartition = \"all\"\n\
         datasets = [\"usage_events\"]\ninterval = \"{interval}\"\n",
        env.home
    )
}

fn csv_rows(env: &Env) -> String {
    let Ok(entries) = std::fs::read_dir(env.home.join("out")) else {
        return String::new();
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .collect()
}

#[tokio::test]
async fn appended_line_reaches_the_store_and_the_csv_sink() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    env.write_config(&csv_sink(&env, "1s"))?;
    let daemon = env.start(fast_timing()).await?;

    assert!(
        eventually(FIVE_SECONDS, || env.usage_events() == 2).await,
        "startup rescan: {} events",
        env.usage_events()
    );
    append(&env.claude_log(), &claude_line(2))?;
    assert!(
        eventually(FIVE_SECONDS, || env.usage_events() == 3).await,
        "appended line: {} events",
        env.usage_events()
    );
    assert!(
        eventually(Duration::from_secs(4), || csv_rows(&env).lines().count()
            == 4)
        .await,
        "csv sink: {}",
        csv_rows(&env)
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn scan_now_and_backup_now_through_the_client() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let daemon = env.start(fast_timing()).await?;
    let mut client = env.client().await?;

    let scan = client.scan_now(None).await?;
    assert_eq!(scan.agents.len(), 2);
    let backup = client.backup_now().await?;
    assert!(backup.path.as_str().ends_with(".db.zst"), "{}", backup.path);
    assert!(backup.path.is_file());
    assert!(backup.size_bytes > 0);

    let status = client.status().await?;
    assert!(status.last_scan_at.is_some());
    assert_eq!(status.tasks, [Task::Watch], "only the watcher is on");
    assert!(client.refresh_quota().await.is_err(), "quota is disabled");
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_shutdown_request_is_answered_then_the_owner_stops_the_daemon() -> TestResult {
    let env = Env::new()?;
    let daemon = env.start(fast_timing()).await?;
    let mut client = env.client().await?;
    client.shutdown().await?;
    tokio::time::timeout(FIVE_SECONDS, daemon.stop_requested()).await?;
    daemon.shutdown().await?;
    assert!(
        !env.transport().socket_path().exists(),
        "the socket is removed"
    );
    Ok(())
}

#[tokio::test]
async fn a_subscriber_gets_one_store_changed_per_append_and_none_when_idle() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let daemon = env.start(fast_timing()).await?;
    assert!(eventually(FIVE_SECONDS, || env.usage_events() == 2).await);
    // Let the startup change be published before subscribing.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut events = env.client().await?.subscribe().await?;

    let idle = tokio::time::timeout(Duration::from_millis(2500), events.next()).await;
    assert!(idle.is_err(), "idle daemon pushed {idle:?}");

    append(&env.claude_log(), &claude_line(2))?;
    let pushed = tokio::time::timeout(Duration::from_secs(1), events.next()).await??;
    let Some(Event::StoreChanged { datasets }) = pushed else {
        return Err(format!("expected StoreChanged, got {pushed:?}").into());
    };
    assert!(datasets.contains(&Dataset::UsageEvents));
    let extra = tokio::time::timeout(Duration::from_millis(2500), events.next()).await;
    assert!(extra.is_err(), "a second event followed: {extra:?}");
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_failing_sink_raises_exactly_one_sink_failing_alert() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    // A regular file where the sink needs a directory: every export fails.
    std::fs::write(env.home.join("blocker"), "x")?;
    env.write_config(&format!(
        "[alerts]\nsink_failing_after = \"2s\"\n\
         [[sinks]]\nid = \"bad\"\nkind = \"csv\"\ndir = \"{}/blocker/out\"\ninterval = \"1s\"\n",
        env.home
    ))?;
    let daemon = env.start(fast_timing()).await?;
    let mut events = env.client().await?.subscribe().await?;

    let alert = loop {
        let event = tokio::time::timeout(Duration::from_secs(12), events.next()).await??;
        if let Some(Event::AlertFired { alert }) = event {
            break alert;
        }
    };
    assert!(
        matches!(alert.kind, AlertKind::SinkFailing { .. }),
        "{alert:?}"
    );

    let more = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(Event::AlertFired { alert }) = events.next().await? {
                return Ok::<_, tokenmtr_control::ClientError>(alert);
            }
        }
    })
    .await;
    assert!(more.is_err(), "a second alert fired: {more:?}");
    let fired = env.reader()?.fired_alert_keys()?;
    let sink_alerts = fired
        .iter()
        .filter(|k| k.starts_with("sink_failing"))
        .count();
    assert_eq!(sink_alerts, 1, "{fired:?}");

    let status = env.client().await?.status().await?;
    let sink = status.sinks.first().ok_or("no sink in status")?;
    assert!(sink.failing_since.is_some());
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_is_fast_and_removes_the_socket() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    env.write_config(&csv_sink(&env, "1s"))?;
    let daemon = env.start(fast_timing()).await?;
    let _subscriber = env.client().await?.subscribe().await?;
    assert!(eventually(FIVE_SECONDS, || env.usage_events() == 2).await);

    let started = Instant::now();
    daemon.shutdown().await?;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(!env.transport().socket_path().exists());
    assert!(env.client().await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_second_daemon_on_the_same_socket_is_refused() -> TestResult {
    let env = Env::new()?;
    let first = env.start(fast_timing()).await?;
    let second = Daemon::start(env.settings(fast_timing())).await;
    assert!(matches!(second, Err(DaemonError::AlreadyRunning(_))));
    // The refused start must not have taken the first daemon's socket.
    assert!(env.client().await?.status().await.is_ok());
    first.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_newer_store_is_refused_and_left_untouched() -> TestResult {
    let env = Env::new()?;
    {
        let _store = tokenmtr_store::Store::open(&env.store_path())?;
    }
    let conn = rusqlite::Connection::open(env.store_path().as_std_path())?;
    conn.pragma_update(None, "user_version", tokenmtr_store::SCHEMA_VERSION + 1)?;
    drop(conn);

    let started = Daemon::start(env.settings(fast_timing())).await;
    assert!(
        matches!(started, Err(DaemonError::NewerSchema { .. })),
        "newer store must be refused"
    );
    assert!(!env.transport().socket_path().exists(), "no stale socket");
    Ok(())
}

#[tokio::test]
async fn an_invalid_config_keeps_the_old_one_until_a_valid_one_arrives() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let daemon = env.start(fast_timing()).await?;
    let mut client = env.client().await?;
    assert!(client.status().await?.config_error.is_none());

    // Picked up by the config watcher, no request needed.
    std::fs::write(&env.config_path, "[quota]\nenabled = \"maybe\"\n")?;
    let mut seen = false;
    for _ in 0..100 {
        if client.status().await?.config_error.is_some() {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seen, "the invalid config was not reported");
    assert!(client.issues().await?.config_error.is_some());
    assert!(client.reload().await.is_err());
    // The old config still drives the daemon: scanning keeps working.
    assert_eq!(client.scan_now(None).await?.agents.len(), 2);

    env.write_config(&csv_sink(&env, "1s"))?;
    client.reload().await?;
    let status = client.status().await?;
    assert!(status.config_error.is_none());
    assert_eq!(status.sinks.len(), 1, "the new config is active");
    daemon.shutdown().await?;
    Ok(())
}
