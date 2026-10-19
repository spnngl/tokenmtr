//! The scrape endpoint: content negotiation, gzip, routing, authentication, live updates.

mod common;

use std::io::Read as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::TestResult;
use flate2::read::GzDecoder;
use jiff::Timestamp;
use reqwest::{Client, Method, Response};
use secrecy::SecretString;
use tokenmtr_metrics::text::Rendered;
use tokenmtr_metrics::{Family, Folded, Series, Value};
use tokenmtr_remote::Credential;
use tokenmtr_remote::scrape::serve;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

fn rendered(snapshot_seconds: f64) -> Arc<Rendered> {
    Arc::new(Rendered::new(&Folded {
        taken_at: Timestamp::UNIX_EPOCH,
        series: vec![Series {
            family: Family::SnapshotTimestamp,
            labels: Vec::new(),
            value: Value::Gauge(snapshot_seconds),
            created: None,
        }],
    }))
}

struct Server {
    addr: SocketAddr,
    publish: watch::Sender<Arc<Rendered>>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
    client: Client,
}

impl Server {
    async fn start(auth: Option<Credential>) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (publish, subscribe) = watch::channel(rendered(1.0));
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            serve(listener, subscribe, auth.as_ref(), async {
                let _ = stopped.await;
            })
            .await
        });
        Ok(Self {
            addr,
            publish,
            stop,
            task,
            client: Client::builder().build()?,
        })
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
    ) -> Result<Response, reqwest::Error> {
        let mut request = self
            .client
            .request(method, format!("http://{}{path}", self.addr));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send().await
    }

    async fn get(&self, headers: &[(&str, &str)]) -> Result<Response, reqwest::Error> {
        self.request(Method::GET, "/metrics", headers).await
    }
}

fn header(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

#[tokio::test]
async fn accept_picks_openmetrics_or_the_plain_format() -> TestResult {
    let server = Server::start(None).await?;
    let plain = server.get(&[]).await?;
    assert_eq!(plain.status(), 200);
    assert_eq!(
        header(&plain, "content-type").as_deref(),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    let text = plain.text().await?;
    assert!(text.contains("tokenmtr_snapshot_timestamp_seconds 1\n") && !text.contains("# EOF"));

    let om = server
        .get(&[(
            "accept",
            "application/openmetrics-text;version=1.0.0,text/plain;version=0.0.4;q=0.5",
        )])
        .await?;
    assert_eq!(
        header(&om, "content-type").as_deref(),
        Some("application/openmetrics-text; version=1.0.0; charset=utf-8")
    );
    assert!(om.text().await?.ends_with("# EOF\n"));
    Ok(())
}

#[tokio::test]
async fn gzip_only_when_the_client_asks() -> TestResult {
    let server = Server::start(None).await?;
    let identity = server.get(&[]).await?;
    assert_eq!(header(&identity, "content-encoding"), None);
    let expected = identity.bytes().await?;

    let zipped = server.get(&[("accept-encoding", "gzip")]).await?;
    assert_eq!(header(&zipped, "content-encoding").as_deref(), Some("gzip"));
    assert_eq!(
        header(&zipped, "vary").as_deref(),
        Some("Accept, Accept-Encoding")
    );
    let mut text = Vec::new();
    GzDecoder::new(zipped.bytes().await?.as_ref()).read_to_end(&mut text)?;
    assert_eq!(text, expected.as_ref());
    Ok(())
}

#[tokio::test]
async fn only_get_metrics_exists() -> TestResult {
    let server = Server::start(None).await?;
    for (method, path) in [
        (Method::GET, "/"),
        (Method::GET, "/metrics/"),
        (Method::GET, "/metrics/x"),
        (Method::GET, "/other"),
        (Method::POST, "/metrics"),
        (Method::HEAD, "/metrics"),
        (Method::DELETE, "/metrics"),
    ] {
        assert_eq!(
            server.request(method.clone(), path, &[]).await?.status(),
            404,
            "{method} {path}"
        );
    }
    assert_eq!(
        server
            .request(Method::GET, "/metrics?x=1", &[])
            .await?
            .status(),
        200
    );
    Ok(())
}

#[tokio::test]
async fn a_new_render_is_served_to_the_next_scrape() -> TestResult {
    let server = Server::start(None).await?;
    assert!(server.get(&[]).await?.text().await?.contains("seconds 1\n"));
    server.publish.send(rendered(2.0))?;
    assert!(server.get(&[]).await?.text().await?.contains("seconds 2\n"));
    Ok(())
}

#[tokio::test]
async fn auth_is_required_and_other_paths_stay_404() -> TestResult {
    for (credential, good, bad, challenge) in [
        (
            Credential::Bearer(SecretString::from("tok-123")),
            "Bearer tok-123",
            [
                "Bearer tok-124",
                "Bearer tok-1234",
                "Basic tok-123",
                "tok-123",
                "",
            ],
            "Bearer",
        ),
        (
            Credential::Basic {
                username: "u".to_owned(),
                password: SecretString::from("p"),
            },
            "Basic dTpw",
            ["Basic dTpx", "Basic dTpwcA==", "Bearer dTpw", "dTpw", ""],
            "Basic",
        ),
    ] {
        let server = Server::start(Some(credential)).await?;
        let missing = server.get(&[]).await?;
        assert_eq!(missing.status(), 401);
        assert!(header(&missing, "www-authenticate").is_some_and(|c| c.starts_with(challenge)));
        for presented in bad {
            let status = server.get(&[("authorization", presented)]).await?.status();
            assert_eq!(status, 401, "{presented:?}");
        }
        assert_eq!(server.get(&[("authorization", good)]).await?.status(), 200);
        assert_eq!(
            server.request(Method::GET, "/", &[]).await?.status(),
            404,
            "no auth challenge off /metrics"
        );
    }
    Ok(())
}

#[tokio::test]
async fn shutdown_ends_the_server_even_with_an_idle_connection() -> TestResult {
    let server = Server::start(None).await?;
    assert_eq!(server.get(&[]).await?.status(), 200); // the client keeps the connection alive
    let Server { stop, task, .. } = server;
    let _ = stop.send(());
    let ended = tokio::time::timeout(Duration::from_secs(5), task).await??;
    assert!(ended.is_ok());
    Ok(())
}
