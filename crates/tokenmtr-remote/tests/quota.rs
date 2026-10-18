//! `ClaudeQuota` against a mock Anthropic server.

mod common;

use common::{FAKE_TOKEN, TestResult, closed_port_base, endpoints, fixture, fixture_text};
use tokenmtr_core::{Plan, WindowId};
use tokenmtr_remote::{
    ClaudeCredentials, ClaudeQuota, QuotaOutcome, RemoteError, build_client, parse_quota_response,
};
use wiremock::matchers::{header, header_regex, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn credentials() -> Result<ClaudeCredentials, Box<dyn std::error::Error>> {
    ClaudeCredentials::parse(&fixture_text("claude_credentials.json")?)?
        .ok_or_else(|| "fixture credentials must hold a token".into())
}

fn quota(server: &MockServer) -> Result<ClaudeQuota, Box<dyn std::error::Error>> {
    Ok(ClaudeQuota::new(
        build_client()?,
        endpoints(server)?.anthropic,
    ))
}

async fn mount_usage(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn ok_maps_every_reported_window() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .and(header(
            "authorization",
            format!("Bearer {FAKE_TOKEN}").as_str(),
        ))
        .and(header("anthropic-beta", "oauth-2025-04-20"))
        .and(header_regex("user-agent", r"^tokenmtr/\d+\.\d+\.\d+"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage.json")?))
        .expect(1)
        .mount(&server)
        .await;

    let poll = quota(&server)?.poll(Some(&credentials()?)).await;

    let QuotaOutcome::Ok(snapshot) = poll.outcome else {
        return Err(format!("expected Ok, got {:?}", poll.outcome).into());
    };
    let ids: Vec<&str> = snapshot.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["five_hour", "seven_day", "seven_day_sonnet"]);
    let five_hour = snapshot
        .window(&WindowId::five_hour())
        .ok_or("five_hour missing")?;
    assert!((five_hour.utilization.percent() - 37.0).abs() < f64::EPSILON);
    assert_eq!(
        five_hour.resets_at,
        Some("2026-05-12T14:59:59.943648Z".parse()?)
    );
    let extra = poll.extra_usage.ok_or("extra_usage missing")?;
    assert!(extra.is_enabled);
    assert_eq!(extra.used_credits, Some(1234.5));
    Ok(())
}

#[tokio::test]
async fn sparse_response_tolerates_missing_windows_and_unknown_fields() -> TestResult {
    let server = MockServer::start().await;
    mount_usage(
        &server,
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage_sparse.json")?),
    )
    .await;

    let poll = quota(&server)?.poll(Some(&credentials()?)).await;

    let QuotaOutcome::Ok(snapshot) = poll.outcome else {
        return Err(format!("expected Ok, got {:?}", poll.outcome).into());
    };
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].state.resets_at, None);
    assert!(poll.extra_usage.is_none());
    Ok(())
}

#[tokio::test]
async fn http_401_is_token_expired() -> TestResult {
    let server = MockServer::start().await;
    mount_usage(
        &server,
        ResponseTemplate::new(401).set_body_string(r#"{"error":{"type":"authentication_error"}}"#),
    )
    .await;

    let poll = quota(&server)?.poll(Some(&credentials()?)).await;

    assert!(matches!(poll.outcome, QuotaOutcome::TokenExpired));
    Ok(())
}

#[tokio::test]
async fn no_credentials_is_signed_out_and_sends_nothing() -> TestResult {
    let server = MockServer::start().await;
    mount_usage(&server, ResponseTemplate::new(200)).await;

    let poll = quota(&server)?.poll(None).await;

    assert!(matches!(poll.outcome, QuotaOutcome::SignedOut));
    let requests = server.received_requests().await.ok_or("no request log")?;
    assert!(requests.is_empty());
    Ok(())
}

#[tokio::test]
async fn bad_bodies_are_malformed() -> TestResult {
    for body in [
        "not json at all",
        "[]",
        r#"{"five_hour": {"utilization": "high"}}"#,
        r#"{"five_hour": {"utilization": -3.0}}"#,
        r#"{"five_hour": {"utilization": 1.0, "resets_at": "tomorrow"}}"#,
    ] {
        let server = MockServer::start().await;
        mount_usage(&server, ResponseTemplate::new(200).set_body_string(body)).await;

        let poll = quota(&server)?.poll(Some(&credentials()?)).await;

        assert!(
            matches!(poll.outcome, QuotaOutcome::Malformed),
            "{body:?} gave {:?}",
            poll.outcome
        );
    }
    Ok(())
}

#[tokio::test]
async fn closed_port_is_network_down() -> TestResult {
    let quota = ClaudeQuota::new(build_client()?, closed_port_base()?);

    let poll = quota.poll(Some(&credentials()?)).await;

    assert!(matches!(poll.outcome, QuotaOutcome::NetworkDown));
    Ok(())
}

#[tokio::test]
async fn server_errors_and_rate_limits_are_network_down() -> TestResult {
    for status in [429, 500, 503] {
        let server = MockServer::start().await;
        mount_usage(&server, ResponseTemplate::new(status)).await;

        let poll = quota(&server)?.poll(Some(&credentials()?)).await;

        assert!(
            matches!(poll.outcome, QuotaOutcome::NetworkDown),
            "{status} gave {:?}",
            poll.outcome
        );
    }
    Ok(())
}

#[tokio::test]
async fn account_merges_credentials_plan_with_profile_identity() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/profile"))
        .and(header(
            "authorization",
            format!("Bearer {FAKE_TOKEN}").as_str(),
        ))
        .and(header("anthropic-beta", "oauth-2025-04-20"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("claude_profile.json")?))
        .mount(&server)
        .await;

    let account = quota(&server)?.fetch_account(&credentials()?).await?;

    assert_eq!(account.email.as_deref(), Some("test.user@example.com"));
    assert_eq!(account.organization.as_deref(), Some("Test Org"));
    assert_eq!(account.plan, Some(Plan::Max20x));
    assert_eq!(
        account.rate_limit_tier.as_deref(),
        Some("default_claude_max_20x")
    );
    Ok(())
}

#[tokio::test]
async fn account_errors_are_typed() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/profile"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let result = quota(&server)?.fetch_account(&credentials()?).await;
    assert!(matches!(result, Err(RemoteError::Status(401))));

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/profile"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    let result = quota(&server)?.fetch_account(&credentials()?).await;
    assert!(matches!(result, Err(RemoteError::Malformed(_))));
    Ok(())
}

#[test]
fn parse_function_never_panics_on_junk() {
    let at = jiff::Timestamp::UNIX_EPOCH;
    for junk in [&b""[..], b"\xff\xfe", b"{", b"null", b"{\"five_hour\":1}"] {
        assert!(parse_quota_response(junk, at).is_err());
    }
}
