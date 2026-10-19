//! Remote write pushes: headers, body, classification of answers, the URL rule.

mod common;

use common::{TestResult, closed_port_base};
use reqwest::Url;
use secrecy::SecretString;
use tokenmtr_remote::remote_write::{PushError, Target, push};
use tokenmtr_remote::{Credential, RemoteError, build_push_client, user_agent};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BODY: &[u8] = b"\x05snappy-bytes";

fn target(
    server: &MockServer,
    tenant: Option<&str>,
    credential: Option<Credential>,
) -> Result<Target, Box<dyn std::error::Error>> {
    let url = Url::parse(&format!("{}/api/v1/push", server.uri()))?;
    Ok(Target::new(url, tenant, credential)?)
}

async fn answer(status: u16) -> Result<Result<(), PushError>, Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    Ok(push(&build_push_client()?, &target(&server, None, None)?, BODY).await)
}

#[tokio::test]
async fn a_push_carries_the_remote_write_headers_the_tenant_and_the_exact_body() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/push"))
        .and(header("content-type", "application/x-protobuf"))
        .and(header("content-encoding", "snappy"))
        .and(header("x-prometheus-remote-write-version", "0.1.0"))
        .and(header("x-scope-orgid", "team-a"))
        .and(header("user-agent", user_agent().as_str()))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let target = target(&server, Some("team-a"), None)?;
    push(&build_push_client()?, &target, BODY).await?;
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].body, BODY);
    assert!(received[0].headers.get("authorization").is_none());
    Ok(())
}

#[tokio::test]
async fn credentials_become_authorization_headers() -> TestResult {
    for (credential, expected) in [
        (Credential::Bearer(SecretString::from("tok")), "Bearer tok"),
        (
            Credential::Basic {
                username: "u".to_owned(),
                password: SecretString::from("p"),
            },
            "Basic dTpw",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", expected))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        push(
            &build_push_client()?,
            &target(&server, None, Some(credential))?,
            BODY,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn throttling_and_server_errors_are_retried() -> TestResult {
    for status in [408, 429, 500, 502, 503, 504] {
        let error = answer(status).await?.err();
        assert!(
            matches!(&error, Some(PushError::Retry(RemoteError::Status(s))) if *s == status),
            "{status}: {error:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn other_refusals_are_dropped() -> TestResult {
    for status in [400, 401, 403, 404, 413, 415, 501] {
        let error = answer(status).await?.err();
        assert!(
            matches!(&error, Some(PushError::Drop(RemoteError::Status(s))) if *s == status),
            "{status}: {error:?}"
        );
    }
    assert!(answer(200).await?.is_ok());
    assert!(answer(204).await?.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_redirect_is_not_followed_and_is_a_refusal() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/push"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/elsewhere"))
        .mount(&server)
        .await;
    Mock::given(path("/elsewhere"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let error = push(&build_push_client()?, &target(&server, None, None)?, BODY)
        .await
        .err();
    assert!(
        matches!(error, Some(PushError::Drop(RemoteError::Status(302)))),
        "{error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_dead_receiver_is_retried() -> TestResult {
    let base = closed_port_base()?;
    let target = Target::new(base.join("/push")?, None, None)?;
    let error = push(&build_push_client()?, &target, BODY).await.err();
    assert!(
        matches!(error, Some(ref e) if e.is_retryable()),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn the_url_is_https_or_loopback() -> TestResult {
    let url = |text: &str| Url::parse(text);
    for ok in [
        "https://prometheus-prod.grafana.net/api/prom/push",
        "http://localhost:9009/push",
        "http://127.0.0.1:9009/push",
        "http://[::1]:9009/push",
    ] {
        assert!(Target::new(url(ok)?, None, None).is_ok(), "{ok}");
    }
    for refused in [
        "http://metrics.example.com/push",
        "http://10.0.0.5/push",
        "ftp://localhost/push",
    ] {
        assert!(
            matches!(
                Target::new(url(refused)?, None, None),
                Err(RemoteError::HostNotAllowed(_))
            ),
            "{refused}"
        );
    }
    assert!(matches!(
        Target::new(url("https://user:pw@example.com/push")?, None, None),
        Err(RemoteError::Malformed(_))
    ));
    assert!(matches!(
        Target::new(url("https://example.com/push")?, Some("a\nb"), None),
        Err(RemoteError::Malformed("tenant"))
    ));
    Ok(())
}
