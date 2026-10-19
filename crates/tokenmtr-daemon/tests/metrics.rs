//! The metrics outputs on a daemon in-process: scrapes, exclusions, reloads, session expiry,
//! retries, secrets and remote write against a mock receiver.

mod common;

use std::net::{SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt as _;
use std::time::Duration;

use common::{
    Env, TestResult, append, claude_line, eventually, eventually_async, fast_timing, seed_logs,
};
use jiff::{SignedDuration, Timestamp};
use tokenmtr_core::SinkId;
use tokenmtr_daemon::Timing;
use tokenmtr_store::StoreRead as _;
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WAIT: Duration = Duration::from_secs(20);

fn timing() -> Timing {
    Timing {
        metrics_backoff: Duration::from_millis(100),
        ..fast_timing()
    }
}

/// The test config with the outputs in `extra` and the shortest refresh interval, so that a
/// scrape follows the first scan within seconds instead of after the default 30 s.
fn configure(env: &Env, extra: &str) -> TestResult {
    env.write_config(&format!("[metrics]\nrefresh_interval = \"5s\"\n{extra}"))
}

/// A port nobody listens on right now.
fn free_port() -> Result<u16, std::io::Error> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn exporter(id: &str, port: u16, extra: &str) -> String {
    format!("[[metrics.exporters]]\nid = \"{id}\"\nlisten = \"127.0.0.1:{port}\"\n{extra}\n")
}

/// The body of `GET /metrics`, or `None` when nothing answers (or it is not a 200).
async fn scrape(port: u16, bearer: Option<&str>) -> Option<String> {
    let client = reqwest::Client::new();
    let mut request = client.get(format!("http://127.0.0.1:{port}/metrics"));
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.ok()?;
    response.status().is_success().then_some(())?;
    response.text().await.ok()
}

async fn has(port: u16, needle: &str) -> bool {
    scrape(port, None)
        .await
        .is_some_and(|body| body.contains(needle))
}

/// The value of the first sample line starting with `prefix` and containing every `labels`.
fn sample(body: &str, prefix: &str, labels: &[&str]) -> Option<f64> {
    body.lines()
        .filter(|line| line.starts_with(prefix))
        .find(|line| labels.iter().all(|l| line.contains(l)))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
}

fn runs(
    env: &Env,
    output: &str,
) -> Result<Vec<tokenmtr_store::MetricsRun>, Box<dyn std::error::Error>> {
    Ok(env
        .reader()?
        .metrics_runs(&SinkId::try_from(output)?, 100)?)
}

fn last_error(env: &Env, output: &str) -> Option<String> {
    runs(env, output)
        .ok()?
        .into_iter()
        .max_by_key(|run| run.started)
        .and_then(|run| run.error)
}

#[tokio::test]
async fn with_no_output_nothing_is_bound_or_recorded() -> TestResult {
    // 9464 is the usual port of other exporters: the check only means something when it is free.
    let default_port_was_free = TcpListener::bind("127.0.0.1:9464").is_ok();
    let env = Env::new()?;
    seed_logs(&env)?;
    let daemon = env.start(timing()).await?;
    assert!(eventually(Duration::from_secs(5), || env.usage_events() == 2).await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    if default_port_was_free {
        assert!(
            TcpListener::bind("127.0.0.1:9464").is_ok(),
            "something bound 9464"
        );
    }
    let count: i64 = rusqlite::Connection::open(env.store_path())?.query_row(
        "SELECT COUNT(*) FROM metrics_run",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(count, 0);
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_scrape_after_a_scan_shows_the_new_tokens() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let port = free_port()?;
    configure(&env, &exporter("local", port, ""))?;
    let daemon = env.start(timing()).await?;
    let claude_output = |body: &str| {
        sample(
            body,
            "tokenmtr_tokens_total{",
            &["agent=\"claude\"", "type=\"output\""],
        )
    };

    assert!(eventually_async(WAIT, || async { has(port, "tokenmtr_build_info{").await }).await);
    assert!(
        eventually_async(WAIT, || async {
            scrape(port, None).await.and_then(|b| claude_output(&b)) == Some(40.0)
        })
        .await,
        "after the startup scan"
    );

    append(&env.claude_log(), &claude_line(2))?;
    assert!(
        eventually_async(WAIT, || async {
            scrape(port, None).await.and_then(|b| claude_output(&b)) == Some(80.0)
        })
        .await,
        "after an appended line"
    );
    // Every other path is a 404, and money never appears.
    let other = reqwest::get(format!("http://127.0.0.1:{port}/")).await?;
    assert_eq!(other.status(), 404);
    let body = scrape(port, None).await.ok_or("no body")?;
    assert!(!body.contains("usd") && !body.contains("cost"), "{body}");
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn two_exporters_with_different_exclusions_differ_by_the_excluded_series() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let (all, filtered) = (free_port()?, free_port()?);
    configure(
        &env,
        &format!(
            "{}{}",
            exporter("all", all, ""),
            exporter("filtered", filtered, "exclude_paths = [\"/tmp/app\"]")
        ),
    )?;
    let daemon = env.start(timing()).await?;

    assert!(eventually_async(WAIT, || has(all, "tokenmtr_tokens_total{")).await);
    assert!(eventually_async(WAIT, || has(filtered, "tokenmtr_build_info{")).await);
    let (a, f) = (
        scrape(all, None).await.ok_or("no body")?,
        scrape(filtered, None).await.ok_or("no body")?,
    );
    assert!(!f.contains("tokenmtr_tokens_total{"), "{f}");
    assert!(!f.contains("tokenmtr_events_total{"), "{f}");
    // What `filtered` still serves is a subset of what `all` serves, apart from the series
    // count and the timestamp of the snapshot.
    for line in f.lines().filter(|l| !l.starts_with('#')) {
        if line.starts_with("tokenmtr_series") || line.starts_with("tokenmtr_snapshot") {
            continue;
        }
        assert!(a.contains(line), "only in filtered: {line}");
    }
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_reload_that_adds_an_exclusion_removes_the_series() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let port = free_port()?;
    configure(&env, &exporter("local", port, ""))?;
    let daemon = env.start(timing()).await?;
    assert!(eventually_async(WAIT, || has(port, "tokenmtr_tokens_total{")).await);

    configure(
        &env,
        &exporter("local", port, "exclude_projects = [\"app\"]"),
    )?;
    env.client().await?.reload().await?;
    assert!(
        eventually_async(WAIT, || async {
            scrape(port, None)
                .await
                .is_some_and(|b| !b.contains("tokenmtr_tokens_total{") && b.contains("build_info"))
        })
        .await,
        "the excluded series are still served"
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_reload_that_changes_only_the_auth_rebinds_the_same_port_without_a_failure() -> TestResult
{
    let env = Env::new()?;
    seed_logs(&env)?;
    let port = free_port()?;
    configure(&env, &exporter("local", port, ""))?;
    let daemon = env.start(timing()).await?;
    assert!(eventually_async(WAIT, || has(port, "tokenmtr_build_info{")).await);

    configure(
        &env,
        &exporter(
            "local",
            port,
            "auth = { kind = \"bearer\", token = \"t0ken\" }",
        ),
    )?;
    env.client().await?.reload().await?;
    assert!(
        eventually_async(WAIT, || async {
            scrape(port, None).await.is_none() && scrape(port, Some("t0ken")).await.is_some()
        })
        .await,
        "the new auth never took over"
    );
    assert!(
        runs(&env, "local")?.iter().all(|r| r.error.is_none()),
        "{:?}",
        runs(&env, "local")?
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_sessions_series_disappears_at_its_expiry_with_no_new_write() -> TestResult {
    let env = Env::new()?;
    // The only event of session s1 is 12 s short of leaving the one-hour window.
    let at = Timestamp::now().checked_sub(SignedDuration::from_secs(3600 - 12))?;
    let line = claude_line(1).replace("2026-03-05T11:00:01.000Z", &at.to_string());
    assert!(line.contains(&at.to_string()));
    std::fs::write(env.claude_log(), line)?;
    let port = free_port()?;
    env.write_config(&format!(
        "[metrics]\nsession_window = \"1h\"\nrefresh_interval = \"5s\"\n{}",
        exporter("local", port, "")
    ))?;
    let daemon = env.start(timing()).await?;

    assert!(
        eventually_async(WAIT, || has(port, "tokenmtr_session_tokens_total{")).await,
        "the session never showed"
    );
    let body = scrape(port, None).await.ok_or("no body")?;
    assert!(body.contains("session_id=\"s1\""), "{body}");
    // No write follows: only the armed expiry can retire the series. The totals stay.
    assert!(
        eventually_async(WAIT, || async {
            scrape(port, None).await.is_some_and(|b| {
                !b.contains("tokenmtr_session_tokens_total{")
                    && b.contains("tokenmtr_tokens_total{")
            })
        })
        .await,
        "the session series outlived its window"
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_busy_port_is_retried_with_back_off_and_binds_once_freed() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let holder = TcpListener::bind("127.0.0.1:0")?;
    let port = holder.local_addr()?.port();
    configure(&env, &exporter("local", port, ""))?;
    let daemon = env.start(timing()).await?;

    assert!(
        eventually(Duration::from_secs(5), || {
            last_error(&env, "local").is_some_and(|e| e.contains("cannot listen"))
        })
        .await,
        "no failed bind recorded"
    );
    let attempts = runs(&env, "local")?.len();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(runs(&env, "local")?.len() > attempts, "no retry");

    drop(holder);
    assert!(eventually_async(WAIT, || has(port, "tokenmtr_build_info{")).await);
    assert!(
        eventually(Duration::from_secs(5), || last_error(&env, "local")
            .is_none())
        .await,
        "the success is recorded"
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn an_exporter_with_an_unset_auth_env_never_binds_while_another_runs() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let (locked, open) = (free_port()?, free_port()?);
    configure(
        &env,
        &format!(
            "[alerts]\nsink_failing_after = \"2s\"\n{}{}",
            exporter(
                "locked",
                locked,
                "auth = { kind = \"bearer\", token_env = \"TOKENMTR_TEST_SURELY_UNSET_9f3\" }"
            ),
            exporter("open", open, "")
        ),
    )?;
    let daemon = env.start(timing()).await?;

    assert!(eventually_async(WAIT, || has(open, "tokenmtr_build_info{")).await);
    assert!(
        eventually(Duration::from_secs(5), || {
            last_error(&env, "locked")
                .is_some_and(|e| e == "secret env TOKENMTR_TEST_SURELY_UNSET_9f3 is unset")
        })
        .await
    );
    assert!(scrape(locked, None).await.is_none(), "it bound anyway");
    assert!(last_error(&env, "open").is_none());

    // It keeps failing past the grace period: one alert, for that output only.
    assert!(
        eventually(Duration::from_secs(12), || {
            env.reader()
                .and_then(|r| Ok(r.fired_alert_keys()?))
                .is_ok_and(|keys| {
                    keys.iter()
                        .any(|k| k.starts_with("metrics_failing|locked|"))
                })
        })
        .await,
        "no metrics_failing alert"
    );
    let fired = env.reader()?.fired_alert_keys()?;
    assert_eq!(
        fired
            .iter()
            .filter(|k| k.starts_with("metrics_failing"))
            .count(),
        1,
        "{fired:?}"
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_file_secret_is_used_and_a_bad_one_fails_the_output_without_leaking_it() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let write = |name: &str, text: &str, mode: u32| -> Result<String, std::io::Error> {
        let path = env.home.join(name);
        std::fs::write(&path, text)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
        Ok(path.to_string())
    };
    let good = write("good.secret", "s3cret-token\n", 0o600)?;
    let open = write("open.secret", "leaky-token-0644", 0o644)?;
    let empty = write("empty.secret", "\n", 0o600)?;
    let missing = env.home.join("missing.secret");
    let ports: Vec<u16> = (0..4).map(|_| free_port()).collect::<Result<_, _>>()?;
    let bearer = |file: &str| format!("auth = {{ kind = \"bearer\", token_file = \"{file}\" }}");
    configure(
        &env,
        &format!(
            "{}{}{}{}",
            exporter("good", ports[0], &bearer(&good)),
            exporter("open", ports[1], &bearer(&open)),
            exporter("empty", ports[2], &bearer(&empty)),
            exporter("missing", ports[3], &bearer(missing.as_str())),
        ),
    )?;
    let daemon = env.start(timing()).await?;

    assert!(
        eventually_async(WAIT, || async {
            scrape(ports[0], Some("s3cret-token")).await.is_some()
        })
        .await
    );
    assert!(scrape(ports[0], None).await.is_none(), "no token, no data");
    assert!(scrape(ports[0], Some("wrong")).await.is_none());
    for (output, expect) in [
        ("open", "readable by others"),
        ("empty", "is empty"),
        ("missing", "not found"),
    ] {
        assert!(
            eventually(Duration::from_secs(5), || {
                last_error(&env, output).is_some_and(|e| e.contains(expect))
            })
            .await,
            "{output}: {:?}",
            last_error(&env, output)
        );
    }
    for (port, output) in ports.iter().skip(1).zip(["open", "empty", "missing"]) {
        assert!(scrape(*port, None).await.is_none(), "{output} bound");
    }
    let stored: String = runs(&env, "open")?
        .into_iter()
        .filter_map(|run| run.error)
        .collect();
    assert!(!stored.contains("leaky-token-0644"), "{stored}");
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_failing_push_backs_off_fires_metrics_failing_and_does_not_block_exports() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let receiver = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&receiver)
        .await;
    configure(
        &env,
        &format!(
            "[alerts]\nsink_failing_after = \"2s\"\n\
         [[sinks]]\nid = \"csv\"\nkind = \"csv\"\ndir = \"{home}/out\"\npartition = \"all\"\n\
         datasets = [\"usage_events\"]\ninterval = \"1s\"\n\
         [[metrics.remote_write]]\nid = \"grafana\"\nurl = \"{url}/api/prom/push\"\ninterval = \"10s\"\n",
            home = env.home,
            url = receiver.uri()
        ),
    )?;
    let daemon = env.start(timing()).await?;

    // Back-off from 100 ms, not one attempt per 10 s interval.
    assert!(
        eventually_async(Duration::from_secs(8), || async {
            receiver
                .received_requests()
                .await
                .is_some_and(|r| r.len() >= 4)
        })
        .await,
        "{:?}",
        receiver.received_requests().await.map(|r| r.len())
    );
    assert!(
        eventually(Duration::from_secs(12), || {
            env.reader()
                .and_then(|r| Ok(r.fired_alert_keys()?))
                .is_ok_and(|keys| {
                    keys.iter()
                        .any(|k| k.starts_with("metrics_failing|grafana|"))
                })
        })
        .await,
        "no metrics_failing alert"
    );
    assert!(last_error(&env, "grafana").is_some_and(|e| e.contains("503")));
    // The csv sink kept exporting the whole time.
    let exported = || {
        std::fs::read_dir(env.home.join("out")).map_or_else(
            |_| String::new(),
            |entries| {
                entries
                    .flatten()
                    .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                    .collect::<String>()
            },
        )
    };
    assert!(
        eventually(Duration::from_secs(5), || exported().lines().count() == 3).await,
        "{}",
        exported()
    );
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_push_carries_the_protocol_headers_the_tenant_and_basic_auth() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    let receiver = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("content-encoding", "snappy"))
        .and(header("x-prometheus-remote-write-version", "0.1.0"))
        .and(header("x-scope-orgid", "team-a"))
        .and(header("authorization", "Basic dToxMjM="))
        .respond_with(ResponseTemplate::new(204))
        .mount(&receiver)
        .await;
    configure(
        &env,
        &format!(
            "[[metrics.remote_write]]\nid = \"grafana\"\nurl = \"{}/push\"\ninterval = \"10s\"\n\
         tenant = \"team-a\"\nauth = {{ kind = \"basic\", username = \"u\", password = \"123\" }}\n",
            receiver.uri()
        ),
    )?;
    let daemon = env.start(timing()).await?;

    assert!(
        eventually_async(WAIT, || async {
            receiver
                .received_requests()
                .await
                .is_some_and(|r| !r.is_empty())
        })
        .await,
        "nothing was pushed"
    );
    assert!(
        eventually(Duration::from_secs(5), || {
            runs(&env, "grafana")
                .is_ok_and(|runs| runs.iter().any(|r| r.error.is_none() && r.series > 0))
        })
        .await,
        "{:?}",
        runs(&env, "grafana")?
    );
    daemon.shutdown().await?;
    // The mock matched every request, so none was rejected for a missing header.
    assert!(runs(&env, "grafana")?.iter().all(|r| r.error.is_none()));
    let _: SocketAddr = receiver.address().to_owned();
    Ok(())
}
