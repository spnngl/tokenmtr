//! `LitellmRefresh` against a mock raw.githubusercontent.com.

mod common;

use common::{TestResult, endpoints, fixture};
use tokenmtr_core::pricing::EMBEDDED_LITELLM;
use tokenmtr_core::{PricingError, PricingVersion};
use tokenmtr_remote::{LitellmRefresh, RefreshOutcome, RemoteError, build_client};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CATALOG: &str = "/BerriAI/litellm/main/model_prices_and_context_window.json";

fn client(server: &MockServer) -> Result<LitellmRefresh, Box<dyn std::error::Error>> {
    Ok(LitellmRefresh::new(
        build_client()?,
        endpoints(server)?.github_raw,
    ))
}

async fn serve(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(CATALOG))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn ok_returns_catalog_body_and_etag() -> TestResult {
    let body = fixture("litellm_small.json")?;
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200)
            .insert_header("etag", "W/\"v1\"")
            .set_body_bytes(body.clone()),
    )
    .await;

    let outcome = client(&server)?.refresh(None).await?;

    let RefreshOutcome::New {
        catalog,
        body: got,
        etag,
    } = outcome
    else {
        return Err(format!("expected New, got {outcome:?}").into());
    };
    assert_eq!(got, body);
    assert_eq!(etag.as_deref(), Some("W/\"v1\""));
    assert_eq!(catalog.version(), &PricingVersion::of_body(&body));
    assert!(!catalog.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_vendored_snapshot_is_accepted() -> TestResult {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200).set_body_bytes(EMBEDDED_LITELLM),
    )
    .await;

    let outcome = client(&server)?.refresh(None).await?;

    assert!(matches!(outcome, RefreshOutcome::New { .. }));
    Ok(())
}

#[tokio::test]
async fn etag_is_replayed_and_304_is_not_modified() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(CATALOG))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;

    let outcome = client(&server)?.refresh(Some("\"v1\"")).await?;

    assert!(matches!(outcome, RefreshOutcome::NotModified));
    Ok(())
}

#[tokio::test]
async fn garbage_body_is_rejected() -> TestResult {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200).set_body_string("<html>rate limited</html>"),
    )
    .await;

    let outcome = client(&server)?.refresh(None).await?;

    assert!(matches!(
        outcome,
        RefreshOutcome::Rejected(PricingError::Json(_))
    ));
    Ok(())
}

#[tokio::test]
async fn valid_json_without_chat_models_is_rejected() -> TestResult {
    let server = MockServer::start().await;
    serve(&server, ResponseTemplate::new(200).set_body_string("{}")).await;

    let outcome = client(&server)?.refresh(None).await?;

    assert!(matches!(
        outcome,
        RefreshOutcome::Rejected(PricingError::NoChatModels)
    ));
    Ok(())
}

#[tokio::test]
async fn http_errors_are_errors_not_rejections() -> TestResult {
    let server = MockServer::start().await;
    serve(&server, ResponseTemplate::new(503)).await;

    let result = client(&server)?.refresh(None).await;

    assert!(matches!(result, Err(RemoteError::Status(503))));
    Ok(())
}
