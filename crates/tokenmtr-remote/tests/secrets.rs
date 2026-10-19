//! The OAuth token never reaches logs, `Debug` output or error strings.

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

const FAKE_PUSH_SECRET: &str = "pw-FAKE-PUSH-SECRET-1111";

fn assert_clean_of(secret: &str, label: &str, text: &str) {
    assert!(!text.contains(secret), "{label} leaks the secret: {text}");
}

#[tokio::test]
async fn push_and_scrape_secrets_never_appear_in_logs_debug_or_errors() -> TestResult {
    use secrecy::SecretString;
    use tokenmtr_metrics::text::Rendered;
    use tokenmtr_remote::remote_write::{Target, push};
    use tokenmtr_remote::{Credential, build_push_client, scrape};

    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .with_ansi(false)
            .finish(),
    );
    let credentials = [
        Credential::Bearer(SecretString::from(FAKE_PUSH_SECRET)),
        Credential::Basic {
            username: "user".to_owned(),
            password: SecretString::from(FAKE_PUSH_SECRET),
        },
    ];
    let mut rendered: Vec<String> = credentials.iter().map(|c| format!("{c:?}")).collect();

    // Receivers that echo the secret back in every kind of answer.
    let echo = format!("denied {FAKE_PUSH_SECRET}");
    for status in [200, 400, 401, 429, 500] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string(echo.clone()))
            .mount(&server)
            .await;
        let url = reqwest::Url::parse(&format!("{}/push", server.uri()))?;
        for credential in &credentials {
            let target = Target::new(url.clone(), Some("tenant"), Some(credential.clone()))?;
            rendered.push(format!("{target:?}"));
            let result = push(&build_push_client()?, &target, b"body").await;
            rendered.push(format!("{result:?}"));
            if let Err(error) = result {
                rendered.push(error.to_string());
            }
        }
    }
    // A dead receiver: transport errors can embed URLs and headers.
    let dead = Target::new(
        closed_port_base()?.join("/push")?,
        None,
        Some(credentials[0].clone()),
    )?;
    if let Err(error) = push(&build_push_client()?, &dead, b"body").await {
        rendered.push(format!("{error:?}"));
        rendered.push(error.to_string());
    }

    // An exporter that requires the secret answers a wrong guess without echoing anything.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (_publish, subscribe) =
        tokio::sync::watch::channel(std::sync::Arc::new(Rendered::default()));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        scrape::serve(listener, subscribe, credentials.first(), async {
            let _ = stopped.await;
        })
        .await
    });
    let client = reqwest::Client::new();
    let answer = client
        .get(format!("http://{addr}/metrics"))
        .header("authorization", format!("Bearer {FAKE_PUSH_SECRET}x"))
        .send()
        .await?;
    rendered.push(format!("{:?}", answer.headers()));
    rendered.push(answer.text().await?);
    let _ = stop.send(());
    server.await??;

    for (index, text) in rendered.iter().enumerate() {
        assert_clean_of(FAKE_PUSH_SECRET, &format!("rendered value #{index}"), text);
    }
    let logs = logs.text();
    assert!(!logs.is_empty(), "the capture saw no tracing events");
    assert_clean_of(FAKE_PUSH_SECRET, "tracing output", &logs);
    Ok(())
}
