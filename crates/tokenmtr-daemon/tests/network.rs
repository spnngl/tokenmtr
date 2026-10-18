//! The networked tasks (quota, FX, price list) against mock servers, and the guarantee that
//! with them off the daemon sends nothing.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{
    Env, TestResult, claude_line_for, eventually, eventually_async, fast_timing, seed_logs,
};
use tokenmtr_core::ApiCost;
use tokenmtr_daemon::Settings;
use tokenmtr_remote::Endpoints;
use tokenmtr_store::{Dataset, Revision, StoreRead as _};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USAGE: &str = "/api/oauth/usage";
const PROFILE: &str = "/api/oauth/profile";
const ECB_DAILY: &str = "/stats/eurofxref/eurofxref-daily.xml";
const ECB_HIST: &str = "/stats/eurofxref/eurofxref-hist.xml";
const CATALOG: &str = "/BerriAI/litellm/main/model_prices_and_context_window.json";
const WAIT: Duration = Duration::from_secs(8);

fn fixture(name: &str) -> Result<Vec<u8>, std::io::Error> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "fixtures",
        "http",
        name,
    ]
    .iter()
    .collect();
    std::fs::read(path)
}

fn settings_for(env: &Env, base: &str) -> Result<Settings, Box<dyn std::error::Error>> {
    let mut settings = env.settings(fast_timing());
    settings.endpoints = Some(Endpoints::at(base)?);
    Ok(settings)
}

async fn serve(server: &MockServer, route: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(response)
        .mount(server)
        .await;
}

fn sign_in(env: &Env) -> TestResult {
    std::fs::write(
        env.home.join("claude/.credentials.json"),
        fixture("claude_credentials.json")?,
    )?;
    Ok(())
}

/// Starts a daemon with quota polling on, polls once through the client, returns the status.
async fn quota_status(env: &Env, base: &str) -> Result<String, Box<dyn std::error::Error>> {
    env.write_config_with(&["quota"], "")?;
    let daemon = tokenmtr_daemon::Daemon::start(settings_for(env, base)?).await?;
    let summary = env.client().await?.refresh_quota().await?;
    daemon.shutdown().await?;
    let profile = summary.profiles.first().ok_or("no profile polled")?;
    Ok(profile.status.clone())
}

#[tokio::test]
async fn quota_ok_stores_snapshots_and_detects_the_plan() -> TestResult {
    let env = Env::new()?;
    sign_in(&env)?;
    let server = MockServer::start().await;
    serve(
        &server,
        USAGE,
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage.json")?),
    )
    .await;
    serve(
        &server,
        PROFILE,
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_profile.json")?),
    )
    .await;

    assert_eq!(quota_status(&env, &server.uri()).await?, "ok");

    let reader = env.reader()?;
    assert!(reader.head_revision(Dataset::QuotaSnapshots)? > Revision::ZERO);
    let accounts = reader.accounts()?;
    assert_eq!(accounts.len(), 1, "{accounts:?}");
    Ok(())
}

#[tokio::test]
async fn quota_failures_are_reported_by_kind() -> TestResult {
    // 401: the token is expired.
    let env = Env::new()?;
    sign_in(&env)?;
    let server = MockServer::start().await;
    serve(&server, USAGE, ResponseTemplate::new(401)).await;
    assert_eq!(quota_status(&env, &server.uri()).await?, "token_expired");

    // No credentials: signed out, and nothing is sent.
    let env = Env::new()?;
    let server = MockServer::start().await;
    serve(&server, USAGE, ResponseTemplate::new(200)).await;
    assert_eq!(quota_status(&env, &server.uri()).await?, "signed_out");
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );

    // A body that is not the documented shape.
    let env = Env::new()?;
    sign_in(&env)?;
    let server = MockServer::start().await;
    serve(
        &server,
        USAGE,
        ResponseTemplate::new(200).set_body_string("<html>"),
    )
    .await;
    assert_eq!(quota_status(&env, &server.uri()).await?, "malformed");

    // Nothing listens.
    let env = Env::new()?;
    sign_in(&env)?;
    assert_eq!(
        quota_status(&env, "http://127.0.0.1:9").await?,
        "network_down"
    );
    Ok(())
}

#[tokio::test]
async fn fx_loads_history_once_then_only_the_daily_file() -> TestResult {
    let env = Env::new()?;
    env.write_config_with(&["fx"], "")?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ECB_HIST))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("ecb_hist.xml")?))
        .expect(1)
        .mount(&server)
        .await;
    let first = tokenmtr_daemon::Daemon::start(settings_for(&env, &server.uri())?).await?;
    assert!(
        eventually(WAIT, || env.head(Dataset::FxRates) > Revision::ZERO).await,
        "no FX history stored"
    );
    first.shutdown().await?;
    server.verify().await;
    server.reset().await;

    Mock::given(method("GET"))
        .and(path(ECB_DAILY))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("ecb_daily.xml")?))
        .expect(1..)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(ECB_HIST))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("ecb_hist.xml")?))
        .expect(0)
        .mount(&server)
        .await;
    let second = tokenmtr_daemon::Daemon::start(settings_for(&env, &server.uri())?).await?;
    let mock = &server;
    let daily_seen = eventually_async(WAIT, || async move {
        mock.received_requests()
            .await
            .is_some_and(|all| all.iter().any(|r| r.url.path() == ECB_DAILY))
    })
    .await;
    assert!(daily_seen, "the daily file was never fetched");
    second.shutdown().await?;
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn pricing_refresh_accepts_a_catalog_and_prices_unpriced_events() -> TestResult {
    let env = Env::new()?;
    std::fs::write(env.claude_log(), claude_line_for(1, "claude-test-sonnet"))?;
    env.write_config_with(&["pricing"], "")?;
    let server = MockServer::start().await;
    serve(
        &server,
        CATALOG,
        ResponseTemplate::new(200)
            .insert_header("etag", "\"v1\"")
            .set_body_bytes(fixture("litellm_small.json")?),
    )
    .await;
    let daemon = tokenmtr_daemon::Daemon::start(settings_for(&env, &server.uri())?).await?;

    let priced = eventually(WAIT, || {
        env.reader()
            .and_then(|r| Ok(r.usage_events_after(Revision::ZERO, 10)?))
            .is_ok_and(|events| {
                events
                    .first()
                    .is_some_and(|e| matches!(e.row.cost.api_cost(), ApiCost::Estimated(_)))
            })
    })
    .await;
    assert!(
        priced,
        "the unpriced event was not priced by the new snapshot"
    );
    let snapshot = env
        .reader()?
        .active_pricing_snapshot()?
        .ok_or("no snapshot")?;
    assert_eq!(snapshot.source.as_deref(), Some("litellm"));
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn pricing_refresh_rejects_a_bad_body_and_keeps_the_old_prices() -> TestResult {
    let env = Env::new()?;
    std::fs::write(env.claude_log(), claude_line_for(1, "claude-test-sonnet"))?;
    env.write_config_with(&["pricing"], "")?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(CATALOG))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"not\": \"a catalog\"}"))
        .expect(1..)
        .mount(&server)
        .await;
    let daemon = tokenmtr_daemon::Daemon::start(settings_for(&env, &server.uri())?).await?;
    assert!(eventually(WAIT, || env.usage_events() == 1).await);
    tokio::time::sleep(Duration::from_secs(1)).await;

    let snapshot = env.reader()?.active_pricing_snapshot()?;
    assert!(
        snapshot.is_none_or(|s| s.source.as_deref() != Some("litellm")),
        "a rejected body must not become a snapshot"
    );
    daemon.shutdown().await?;
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn with_every_networked_task_off_nothing_is_sent() -> TestResult {
    let env = Env::new()?;
    seed_logs(&env)?;
    sign_in(&env)?;
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    // The default test config has quota, fx and pricing off.
    let daemon = tokenmtr_daemon::Daemon::start(settings_for(&env, &server.uri())?).await?;
    assert!(eventually(WAIT, || env.usage_events() == 2).await);
    tokio::time::sleep(Duration::from_secs(1)).await;
    daemon.shutdown().await?;
    server.verify().await;
    Ok(())
}
