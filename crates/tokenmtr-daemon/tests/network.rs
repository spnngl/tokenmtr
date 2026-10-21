//! The networked tasks (quota, FX, price list) against mock servers, and the guarantee that
//! with them off the daemon sends nothing.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{
    Env, TestResult, WAIT, claude_line_for, eventually, eventually_async, fast_timing, seed_logs,
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

/// Starts a daemon with quota polling on, polls once through the client, returns the status as
/// it goes on the wire.
async fn quota_status(env: &Env, base: &str) -> Result<String, Box<dyn std::error::Error>> {
    env.write_config_with(&["quota"], "")?;
    let daemon = tokenmtr_daemon::Daemon::start(settings_for(env, base)?).await?;
    let summary = env.client().await?.refresh_quota().await?;
    daemon.shutdown().await?;
    let profile = summary.profiles.first().ok_or("no profile polled")?;
    let wire = serde_json::to_value(profile.status)?;
    Ok(wire.as_str().ok_or("status is not a string")?.to_owned())
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

    // Nothing listens. The daemon's own first poll can fail before the client's refresh asks,
    // because a refused connection returns at once; the refresh then meets that poll's backoff.
    let env = Env::new()?;
    sign_in(&env)?;
    let status = quota_status(&env, "http://127.0.0.1:9").await?;
    assert!(
        matches!(status.as_str(), "network_down" | "backoff"),
        "{status}"
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
    // The embedded list plus the test model: a download must keep the anchors and the models
    // of the active snapshot to be accepted.
    let mut catalog: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(tokenmtr_core::pricing::EMBEDDED_LITELLM)?;
    let small: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&fixture("litellm_small.json")?)?;
    catalog.extend(small);
    let server = MockServer::start().await;
    serve(
        &server,
        CATALOG,
        ResponseTemplate::new(200)
            .insert_header("etag", "\"v1\"")
            .set_body_bytes(serde_json::to_vec(&catalog)?),
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
    assert_eq!(
        snapshot.etag.as_deref(),
        Some("\"v1\""),
        "the ETag is stored"
    );
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

#[tokio::test]
async fn a_refreshed_token_is_used_at_the_next_poll() -> TestResult {
    let env = Env::new()?;
    sign_in(&env)?;
    let server = MockServer::start().await;
    serve(
        &server,
        USAGE,
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage.json")?),
    )
    .await;
    serve(&server, PROFILE, ResponseTemplate::new(404)).await;
    env.write_config_with(&["quota"], "")?;
    let daemon = tokenmtr_daemon::Daemon::start(settings_for(&env, &server.uri())?).await?;
    let mut client = env.client().await?;
    client.refresh_quota().await?;

    // The agent refreshes its token while the cached one is still valid.
    let refreshed = String::from_utf8(fixture("claude_credentials.json")?)?
        .replace("FAKE-TOKEN", "FRESH-TOKEN");
    std::fs::write(env.home.join("claude/.credentials.json"), refreshed)?;
    client.refresh_quota().await?;

    let bearers: Vec<String> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.url.path() == USAGE)
        .filter_map(|request| {
            request
                .headers
                .get("authorization")?
                .to_str()
                .ok()
                .map(str::to_owned)
        })
        .collect();
    assert!(
        bearers.first().is_some_and(|b| b.contains("FAKE-TOKEN")),
        "{bearers:?}"
    );
    assert!(
        bearers.last().is_some_and(|b| b.contains("FRESH-TOKEN")),
        "{bearers:?}"
    );
    daemon.shutdown().await?;
    Ok(())
}

/// How [`ScriptedKeychain`] answers its next quiet lookups.
#[derive(Clone, Copy)]
enum Answer {
    Grant,
    Fail,
    Deny,
}

/// A Keychain whose quiet lookups answer as the test sets, and that never prompts.
struct ScriptedKeychain {
    credentials: String,
    answer: std::sync::Mutex<Answer>,
}

impl ScriptedKeychain {
    fn answer(&self, answer: Answer) {
        *self
            .answer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = answer;
    }
}

impl tokenmtr_platform::SecretStore for ScriptedKeychain {
    fn get(
        &self,
        _service: &str,
    ) -> Result<Option<secrecy::SecretString>, tokenmtr_platform::SecretError> {
        match *self
            .answer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Answer::Grant => Ok(Some(secrecy::SecretString::from(self.credentials.clone()))),
            Answer::Fail => Err(tokenmtr_platform::SecretError::Store("locked".into())),
            Answer::Deny => Err(tokenmtr_platform::SecretError::Denied),
        }
    }
}

#[tokio::test]
async fn a_failed_or_denied_read_keeps_the_cached_token() -> TestResult {
    let env = Env::new()?;
    let server = MockServer::start().await;
    serve(
        &server,
        USAGE,
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage.json")?),
    )
    .await;
    serve(&server, PROFILE, ResponseTemplate::new(404)).await;
    env.write_config_with(&["quota"], "")?;
    let keychain = std::sync::Arc::new(ScriptedKeychain {
        credentials: String::from_utf8(fixture("claude_credentials.json")?)?,
        answer: std::sync::Mutex::new(Answer::Grant),
    });
    let mut settings = settings_for(&env, &server.uri())?;
    settings.secrets = keychain.clone();
    let daemon = tokenmtr_daemon::Daemon::start(settings).await?;
    let mut client = env.client().await?;
    let status = |summary: &tokenmtr_control::QuotaSummary| {
        summary
            .profiles
            .first()
            .map(|p| serde_json::to_value(p.status).unwrap_or_default())
    };

    // Granted, then failing, then denied, then read from files only (none): the server keeps
    // answering the token read first.
    for answer in [Answer::Grant, Answer::Fail, Answer::Deny, Answer::Deny] {
        keychain.answer(answer);
        let summary = client.refresh_quota().await?;
        assert_eq!(status(&summary), Some("ok".into()));
    }
    daemon.shutdown().await?;
    Ok(())
}

/// A Keychain that refuses quiet lookups and grants the one the user answers.
struct PromptingKeychain(String);

impl tokenmtr_platform::SecretStore for PromptingKeychain {
    fn get(
        &self,
        _service: &str,
    ) -> Result<Option<secrecy::SecretString>, tokenmtr_platform::SecretError> {
        Err(tokenmtr_platform::SecretError::Denied)
    }

    fn get_interactive(
        &self,
        _service: &str,
    ) -> Result<Option<secrecy::SecretString>, tokenmtr_platform::SecretError> {
        Ok(Some(secrecy::SecretString::from(self.0.clone())))
    }
}

/// A Keychain that grants quiet lookups and refuses every prompt.
struct QuietKeychain(String);

impl tokenmtr_platform::SecretStore for QuietKeychain {
    fn get(
        &self,
        _service: &str,
    ) -> Result<Option<secrecy::SecretString>, tokenmtr_platform::SecretError> {
        Ok(Some(secrecy::SecretString::from(self.0.clone())))
    }

    fn get_interactive(
        &self,
        _service: &str,
    ) -> Result<Option<secrecy::SecretString>, tokenmtr_platform::SecretError> {
        Err(tokenmtr_platform::SecretError::Denied)
    }
}

#[tokio::test]
async fn connecting_a_readable_keychain_shows_no_prompt() -> TestResult {
    let env = Env::new()?;
    let server = MockServer::start().await;
    serve(
        &server,
        USAGE,
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage.json")?),
    )
    .await;
    serve(&server, PROFILE, ResponseTemplate::new(404)).await;
    env.write_config_with(&["quota"], "")?;
    let mut settings = settings_for(&env, &server.uri())?;
    let credentials = String::from_utf8(fixture("claude_credentials.json")?)?;
    settings.secrets = std::sync::Arc::new(QuietKeychain(credentials));
    let daemon = tokenmtr_daemon::Daemon::start(settings).await?;
    let mut client = env.client().await?;

    let summary = client.connect_keychain().await?;
    let status = summary
        .profiles
        .first()
        .map(|p| serde_json::to_value(p.status).unwrap_or_default());
    assert_eq!(status, Some("ok".into()));
    daemon.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_denied_keychain_stays_denied_until_connected() -> TestResult {
    let env = Env::new()?;
    let server = MockServer::start().await;
    serve(
        &server,
        USAGE,
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage.json")?),
    )
    .await;
    serve(&server, PROFILE, ResponseTemplate::new(404)).await;
    env.write_config_with(&["quota"], "")?;
    let mut settings = settings_for(&env, &server.uri())?;
    let credentials = String::from_utf8(fixture("claude_credentials.json")?)?;
    settings.secrets = std::sync::Arc::new(PromptingKeychain(credentials));
    let daemon = tokenmtr_daemon::Daemon::start(settings).await?;
    let mut client = env.client().await?;
    let status = |summary: &tokenmtr_control::QuotaSummary| {
        summary
            .profiles
            .first()
            .map(|p| serde_json::to_value(p.status).unwrap_or_default())
    };

    // Denied, then files only: still denied, never "signed out".
    for _ in 0..2 {
        let summary = client.refresh_quota().await?;
        assert_eq!(status(&summary), Some("keychain_denied".into()));
    }
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    let summary = client.connect_keychain().await?;
    assert_eq!(status(&summary), Some("ok".into()));
    daemon.shutdown().await?;
    Ok(())
}
