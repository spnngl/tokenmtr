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

/// Prices the three anchors and nothing else.
const ANCHORED: &str = r#"{
  "claude-opus-4-7": {"mode": "chat", "input_cost_per_token": 5e-06, "output_cost_per_token": 2.5e-05},
  "claude-sonnet-4-5": {"mode": "chat", "input_cost_per_token": 3e-06, "output_cost_per_token": 1.5e-05},
  "gpt-5": {"mode": "responses", "input_cost_per_token": 1.25e-06, "output_cost_per_token": 1e-05}
}"#;

#[tokio::test]
async fn ok_returns_catalog_body_and_etag() -> TestResult {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200)
            .insert_header("etag", "W/\"v1\"")
            .set_body_bytes(EMBEDDED_LITELLM),
    )
    .await;

    let outcome = client(&server)?
        .refresh(None, Some(EMBEDDED_LITELLM))
        .await?;

    let RefreshOutcome::New {
        catalog,
        body: got,
        etag,
    } = outcome
    else {
        return Err(format!("expected New, got {outcome:?}").into());
    };
    assert_eq!(got, EMBEDDED_LITELLM);
    assert_eq!(etag.as_deref(), Some("W/\"v1\""));
    assert_eq!(
        catalog.version(),
        &PricingVersion::of_body(EMBEDDED_LITELLM)
    );
    assert_eq!(catalog.carried(), 0);
    Ok(())
}

#[tokio::test]
async fn models_the_download_dropped_are_carried_forward() -> TestResult {
    let active = fixture("litellm_small.json")?;
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200).set_body_bytes(EMBEDDED_LITELLM),
    )
    .await;

    let outcome = client(&server)?.refresh(None, Some(&active)).await?;

    let RefreshOutcome::New { catalog, body, .. } = outcome else {
        return Err(format!("expected New, got {outcome:?}").into());
    };
    assert_eq!(catalog.carried(), 1);
    assert_eq!(catalog.version(), &PricingVersion::of_body(&body));
    assert_ne!(body, EMBEDDED_LITELLM);
    Ok(())
}

#[tokio::test]
async fn a_catalog_without_the_anchor_models_is_rejected() -> TestResult {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200).set_body_bytes(fixture("litellm_small.json")?),
    )
    .await;

    let outcome = client(&server)?.refresh(None, None).await?;

    assert!(matches!(
        outcome,
        RefreshOutcome::Rejected(PricingError::MissingAnchor { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_catalog_with_half_the_models_gone_is_rejected() -> TestResult {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(200).set_body_string(ANCHORED),
    )
    .await;

    let shrunk = client(&server)?
        .refresh(None, Some(EMBEDDED_LITELLM))
        .await?;
    let first = client(&server)?.refresh(None, None).await?;

    assert!(matches!(
        shrunk,
        RefreshOutcome::Rejected(PricingError::Shrunk { models: 3, .. })
    ));
    assert!(matches!(first, RefreshOutcome::New { .. }));
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

    let outcome = client(&server)?.refresh(Some("\"v1\""), None).await?;

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

    let outcome = client(&server)?.refresh(None, None).await?;

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

    let outcome = client(&server)?.refresh(None, None).await?;

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

    let result = client(&server)?.refresh(None, None).await;

    assert!(matches!(result, Err(RemoteError::Status(503))));
    Ok(())
}
