use std::time::Duration;

use camino::Utf8PathBuf;
use tokenmtr_platform::{ControlTransport, LineReader, LineWriter};
use tokio::sync::watch;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Echo;

impl Handler for Echo {
    async fn handle(&self, request: Request) -> Response {
        match request {
            Request::Reload => Response::Error {
                kind: ErrorKind::InvalidConfig,
                message: "nope".to_owned(),
            },
            Request::BackupNow => Response::BackedUp(BackupSummary {
                path: Utf8PathBuf::from("/tmp/x.db.zst"),
                size_bytes: 7,
            }),
            Request::Shutdown => Response::ShuttingDown,
            // Never answers, like a wedged daemon.
            Request::Issues => std::future::pending().await,
            _ => Response::Reloaded,
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    transport: ControlTransport,
    bus: EventBus,
    stop: watch::Sender<bool>,
    server: tokio::task::JoinHandle<tokenmtr_platform::control::DaemonLock>,
}

async fn start() -> Result<Fixture, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = Utf8PathBuf::from_path_buf(dir.path().join("control.sock"))
        .map_err(|p| format!("non-UTF-8 temp path {}", p.display()))?;
    let transport = ControlTransport::unix(path);
    let listener = transport.bind().await?;
    let bus = EventBus::new();
    let (stop, shutdown) = watch::channel(false);
    let info = ServerInfo {
        daemon_version: "9.9.9".to_owned(),
        store_schema: 3,
    };
    let server = tokio::spawn(Server::new(listener, Echo, info, bus.clone()).serve(shutdown));
    Ok(Fixture {
        _dir: dir,
        transport,
        bus,
        stop,
        server,
    })
}

fn me() -> ClientInfo {
    ClientInfo {
        name: "test".to_owned(),
        version: "0".to_owned(),
        schema: Some(3),
    }
}

#[tokio::test]
async fn handshake_then_requests() -> TestResult {
    let fx = start().await?;
    let mut client = Client::connect(&fx.transport, me()).await?;
    assert_eq!(client.daemon().daemon_version, "9.9.9");
    assert_eq!(client.daemon().store_schema, 3);
    assert_eq!(client.backup_now().await?.size_bytes, 7);
    let err = client.reload().await.err().ok_or("expected an error")?;
    assert!(matches!(
        err,
        ClientError::Remote {
            kind: ErrorKind::InvalidConfig,
            ..
        }
    ));
    // The connection survives a daemon-side error.
    assert_eq!(client.backup_now().await?.size_bytes, 7);
    client.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn other_protocol_gets_incompatible_and_no_data() -> TestResult {
    let fx = start().await?;
    let (read, write) = fx.transport.connect().await?.into_split();
    let mut reader = LineReader::new(read);
    let mut writer = LineWriter::new(write);
    // Unknown extra fields must not matter: the probe only reads `type` and `protocol`.
    writer
        .write_line(r#"{"type":"hello","protocol":99,"client":{"anything":1},"future":true}"#)
        .await?;
    let line = reader.read_line().await?.ok_or("no answer")?;
    let response: Response = serde_json::from_str(&line)?;
    let Response::Incompatible {
        daemon_protocol,
        daemon_version,
        store_schema,
        ..
    } = response
    else {
        return Err(format!("expected incompatible, got {response:?}").into());
    };
    assert_eq!(
        (daemon_protocol, daemon_version.as_str(), store_schema),
        (PROTOCOL_VERSION, "9.9.9", 3)
    );
    // The daemon closes the connection and answers nothing else.
    writer.write_line(r#"{"type":"status"}"#).await.ok();
    assert_eq!(reader.read_line().await?, None);
    Ok(())
}

#[tokio::test]
async fn first_message_must_be_hello() -> TestResult {
    let fx = start().await?;
    let (read, write) = fx.transport.connect().await?.into_split();
    let mut reader = LineReader::new(read);
    LineWriter::new(write)
        .write_line(r#"{"type":"status"}"#)
        .await?;
    let line = reader.read_line().await?.ok_or("no answer")?;
    assert!(matches!(
        serde_json::from_str::<Response>(&line)?,
        Response::Error {
            kind: ErrorKind::BadRequest,
            ..
        }
    ));
    assert_eq!(reader.read_line().await?, None);
    Ok(())
}

#[tokio::test]
async fn subscriber_gets_published_events_only() -> TestResult {
    let fx = start().await?;
    let mut subscription = Client::connect(&fx.transport, me())
        .await?
        .subscribe()
        .await?;
    assert_eq!(fx.bus.subscribers(), 1);
    // Nothing is pushed while nothing is published.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), subscription.next())
            .await
            .is_err()
    );
    let event = Event::StoreChanged {
        datasets: vec![Dataset::UsageEvents],
    };
    fx.bus.publish(event.clone());
    let got = tokio::time::timeout(Duration::from_secs(1), subscription.next()).await??;
    assert_eq!(got, Some(event));
    Ok(())
}

#[tokio::test]
async fn shutdown_closes_connections_and_stops_serving() -> TestResult {
    let fx = start().await?;
    let mut subscription = Client::connect(&fx.transport, me())
        .await?
        .subscribe()
        .await?;
    fx.stop.send(true)?;
    tokio::time::timeout(Duration::from_secs(2), fx.server).await??;
    assert_eq!(subscription.next().await?, None);
    Ok(())
}

#[tokio::test]
async fn a_pending_request_yields_to_shutdown() -> TestResult {
    let fx = start().await?;
    let mut client = Client::connect(&fx.transport, me()).await?;
    let pending = tokio::spawn(async move { client.issues().await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    fx.stop.send(true)?;
    tokio::time::timeout(Duration::from_secs(2), fx.server).await??;
    let err = pending.await?.err().ok_or("expected an error")?;
    assert!(
        matches!(
            err,
            ClientError::Remote {
                kind: ErrorKind::ShuttingDown,
                ..
            }
        ),
        "{err}"
    );
    Ok(())
}

#[tokio::test]
async fn a_daemon_that_never_greets_times_out() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = Utf8PathBuf::from_path_buf(dir.path().join("control.sock"))
        .map_err(|p| format!("non-UTF-8 temp path {}", p.display()))?;
    // Connections queue in the backlog but nobody ever reads them.
    let _silent = std::os::unix::net::UnixListener::bind(&path)?;
    let started = std::time::Instant::now();
    let err = Client::connect(&ControlTransport::unix(path), me())
        .await
        .err()
        .ok_or("expected a timeout")?;
    assert!(matches!(err, ClientError::Timeout(_)), "{err}");
    assert!(started.elapsed() < Duration::from_secs(4));
    Ok(())
}

#[test]
fn messages_are_tagged_snake_case_objects() -> TestResult {
    let line = serde_json::to_string(&Request::ScanNow { agent: None })?;
    assert_eq!(line, r#"{"type":"scan_now","agent":null}"#);
    let line = serde_json::to_string(&Response::Event {
        event: Event::StoreChanged {
            datasets: vec![Dataset::QuotaSnapshots],
        },
    })?;
    assert_eq!(
        line,
        r#"{"type":"event","event":{"type":"store_changed","datasets":["quota_snapshots"]}}"#
    );
    Ok(())
}

#[test]
fn every_message_round_trips() -> TestResult {
    let requests = [
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            client: me(),
        },
        Request::Status,
        Request::ScanNow { agent: None },
        Request::ExportNow { sink: None },
        Request::BackupNow,
        Request::Reprice { all: true },
        Request::RefreshQuota,
        Request::ConnectKeychain,
        Request::Reload,
        Request::Issues,
        Request::Subscribe,
        Request::Shutdown,
    ];
    for request in requests {
        let back: Request = serde_json::from_str(&serde_json::to_string(&request)?)?;
        assert_eq!(back, request);
    }
    let responses = [
        Response::Welcome {
            protocol: 1,
            daemon_version: "1".to_owned(),
            store_schema: 2,
        },
        Response::Reloaded,
        Response::ShuttingDown,
        Response::Subscribed,
        Response::Scanned(ScanSummary {
            agents: vec![],
            elapsed_ms: 4,
        }),
        Response::Issues(IssueList {
            parse_issues: vec![],
            config_error: Some("bad".to_owned()),
        }),
        Response::Repriced(RepriceSummary { repriced: 3 }),
        Response::Error {
            kind: ErrorKind::Disabled,
            message: "off".to_owned(),
        },
    ];
    for response in responses {
        let back: Response = serde_json::from_str(&serde_json::to_string(&response)?)?;
        assert_eq!(back, response);
    }
    Ok(())
}
