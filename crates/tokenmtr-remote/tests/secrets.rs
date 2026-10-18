//! D55: the OAuth token never reaches logs, `Debug` output or error strings.

mod common;

use std::io;
use std::sync::{Arc, Mutex};

use common::{FAKE_TOKEN, TestResult, closed_port_base, endpoints, fixture, fixture_text};
use tokenmtr_remote::{
    ClaudeCredentials, ClaudeQuota, CredentialLocation, build_client, parse_quota_response,
};
use tracing_subscriber::fmt::MakeWriter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A log sink the test can read back.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        let bytes = self.0.lock().map(|b| b.clone()).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn assert_clean(label: &str, text: &str) {
    assert!(
        !text.contains(FAKE_TOKEN),
        "{label} leaks the token: {text}"
    );
    assert!(
        !text.contains("FAKE-TOKEN"),
        "{label} leaks a token fragment: {text}"
    );
}

#[tokio::test]
async fn the_token_never_appears_in_logs_debug_or_errors() -> TestResult {
    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .with_ansi(false)
            .finish(),
    );
    let credentials = ClaudeCredentials::parse(&fixture_text("claude_credentials.json")?)?
        .ok_or("fixture credentials must hold a token")?;
    let mut rendered = vec![
        format!("{credentials:?}"),
        format!("{:?}", credentials.access_token()),
    ];

    // Every response shape, including bodies that echo the token back.
    let echo = format!(r#"{{"error":"bad token {FAKE_TOKEN}"}}"#);
    let responses = [
        ResponseTemplate::new(200).set_body_bytes(fixture("claude_usage.json")?),
        ResponseTemplate::new(200).set_body_string(echo.clone()),
        ResponseTemplate::new(401).set_body_string(echo.clone()),
        ResponseTemplate::new(500).set_body_string(echo.clone()),
        ResponseTemplate::new(403).set_body_string(echo),
    ];
    for response in responses {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/oauth/usage"))
            .respond_with(response)
            .mount(&server)
            .await;
        let quota = ClaudeQuota::new(build_client()?, endpoints(&server)?.anthropic);
        let poll = quota.poll(Some(&credentials)).await;
        rendered.push(format!("{poll:?}"));
        let profile = quota.fetch_account(&credentials).await;
        rendered.push(format!("{profile:?}"));
        if let Err(error) = profile {
            rendered.push(error.to_string());
        }
    }

    // A dead endpoint, so transport errors (which can embed URLs and headers) are exercised.
    let dead = ClaudeQuota::new(build_client()?, closed_port_base()?);
    rendered.push(format!("{:?}", dead.poll(Some(&credentials)).await));
    if let Err(error) = dead.fetch_account(&credentials).await {
        rendered.push(error.to_string());
        rendered.push(format!("{error:?}"));
    }

    // Parse failures on a document that contains the token.
    let document = format!(r#"{{"five_hour":{{"utilization":"{FAKE_TOKEN}"}}}}"#);
    if let Err(error) = parse_quota_response(document.as_bytes(), jiff::Timestamp::UNIX_EPOCH) {
        rendered.push(error.to_string());
        rendered.push(format!("{error:?}"));
    }

    // Credential files: unreadable-as-JSON content that includes the token.
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join(".credentials.json"),
        format!(r#"{{"claudeAiOauth":{{"accessToken":"{FAKE_TOKEN}","#),
    )?;
    let config_dir = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
        .map_err(|p| format!("non-UTF-8 temp dir {}", p.display()))?;
    let location = CredentialLocation::config_dir(&config_dir);
    rendered.push(format!("{location:?}"));
    let loaded = ClaudeCredentials::load(&location, &NoSecrets);
    if let Err(error) = loaded {
        rendered.push(error.to_string());
        rendered.push(format!("{error:?}"));
    }
    if let Err(error) = ClaudeCredentials::parse(&format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{FAKE_TOKEN}""#
    )) {
        rendered.push(error.to_string());
        rendered.push(format!("{error:?}"));
    }

    for (index, text) in rendered.iter().enumerate() {
        assert_clean(&format!("rendered value #{index}"), text);
    }
    let logs = logs.text();
    assert!(!logs.is_empty(), "the capture saw no tracing events");
    assert_clean("tracing output", &logs);
    Ok(())
}

/// A secret store with nothing in it.
struct NoSecrets;

impl tokenmtr_platform::SecretStore for NoSecrets {
    fn get(
        &self,
        _service: &str,
    ) -> Result<Option<secrecy::SecretString>, tokenmtr_platform::SecretError> {
        Ok(None)
    }
}
