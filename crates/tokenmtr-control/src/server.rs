//! Server building block: accept loop, handshake, event fan-out.
//!
//! The daemon implements [`Handler`] for the request kinds with side effects and publishes
//! [`Event`]s on an [`EventBus`]. Everything protocol-level lives here: the `Hello` handshake
//! (D52), `Subscribe`, framing and shutdown.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokenmtr_platform::{ControlListener, ControlStream, LineReader, LineWriter};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinSet;

use crate::protocol::{Dataset, ErrorKind, Event, PROTOCOL_VERSION, Request, Response};

/// A client that has not completed the handshake after this long is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Events buffered per subscriber before it is told to resynchronize.
const EVENT_BACKLOG: usize = 64;

/// Flips to `true` when the daemon shuts down. A dropped sender counts as shutdown.
pub type Shutdown = watch::Receiver<bool>;

/// Answers the requests that need daemon state. The server calls it for every request except
/// `Hello` and `Subscribe`.
pub trait Handler: Send + Sync + 'static {
    /// Handles one request. Failures are [`Response::Error`], never panics.
    fn handle(&self, request: Request) -> impl Future<Output = Response> + Send;
}

/// What the server tells clients about the daemon in `Welcome` and `Incompatible`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// The daemon's package version.
    pub daemon_version: String,
    /// The store schema version the daemon runs.
    pub store_schema: u32,
}

/// Fan-out of [`Event`]s to subscribed connections. Cheap to clone.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    /// A bus with no subscribers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sender: broadcast::channel(EVENT_BACKLOG).0,
        }
    }

    /// Sends `event` to every subscriber; a no-op without subscribers.
    pub fn publish(&self, event: Event) {
        // `Err` only means nobody listens.
        let _ = self.sender.send(event);
    }

    /// Connections currently subscribed.
    #[must_use]
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

/// The control server: accepts connections on a bound listener until shutdown.
#[derive(Debug)]
pub struct Server<H> {
    listener: ControlListener,
    handler: Arc<H>,
    info: ServerInfo,
    bus: EventBus,
}

impl<H: Handler> Server<H> {
    /// A server over a listener from `ControlTransport::bind`.
    #[must_use]
    pub fn new(listener: ControlListener, handler: H, info: ServerInfo, bus: EventBus) -> Self {
        Self {
            listener,
            handler: Arc::new(handler),
            info,
            bus,
        }
    }

    /// Accepts connections until `shutdown` flips, then closes every connection and returns.
    pub async fn serve(self, mut shutdown: Shutdown) {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                () = wait_shutdown(&mut shutdown) => break,
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        connections.spawn(connection(
                            stream,
                            Arc::clone(&self.handler),
                            self.info.clone(),
                            self.bus.clone(),
                            shutdown.clone(),
                        ));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "control accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        drop(self.listener);
        while connections.join_next().await.is_some() {}
    }
}

/// Resolves once shutdown was requested (or the daemon dropped its sender).
pub async fn wait_shutdown(shutdown: &mut Shutdown) {
    let _ = shutdown.wait_for(|stop| *stop).await;
}

/// First line of a connection; read leniently so a future protocol's `Hello` still gets an
/// `Incompatible` answer instead of a parse error.
#[derive(Debug, Deserialize)]
struct Probe {
    #[serde(rename = "type")]
    kind: String,
    protocol: Option<u32>,
}

type Reader = LineReader<tokio::net::unix::OwnedReadHalf>;
type Writer = LineWriter<tokio::net::unix::OwnedWriteHalf>;

async fn send(writer: &mut Writer, response: &Response) -> bool {
    let line = match serde_json::to_string(response) {
        Ok(line) => line,
        Err(error) => {
            tracing::error!(%error, "control response is not serializable");
            return false;
        }
    };
    match writer.write_line(&line).await {
        Ok(()) => true,
        Err(error) => {
            tracing::debug!(%error, "control client went away");
            false
        }
    }
}

fn bad_request(message: &str) -> Response {
    Response::Error {
        kind: ErrorKind::BadRequest,
        message: message.to_owned(),
    }
}

async fn handshake(reader: &mut Reader, writer: &mut Writer, info: &ServerInfo) -> bool {
    let line = match tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.read_line()).await {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) | Err(_) => return false,
        Ok(Err(error)) => {
            tracing::debug!(%error, "control handshake failed");
            return false;
        }
    };
    let Ok(probe) = serde_json::from_str::<Probe>(&line) else {
        send(writer, &bad_request("the first message must be `hello`")).await;
        return false;
    };
    if probe.kind != "hello" {
        send(writer, &bad_request("the first message must be `hello`")).await;
        return false;
    }
    let client_protocol = probe.protocol.unwrap_or(0);
    if client_protocol != PROTOCOL_VERSION {
        send(
            writer,
            &Response::Incompatible {
                daemon_protocol: PROTOCOL_VERSION,
                daemon_version: info.daemon_version.clone(),
                store_schema: info.store_schema,
                reason: format!(
                    "the client speaks control protocol {client_protocol}, the daemon {PROTOCOL_VERSION}"
                ),
            },
        )
        .await;
        return false;
    }
    if !matches!(
        serde_json::from_str::<Request>(&line),
        Ok(Request::Hello { .. })
    ) {
        send(writer, &bad_request("malformed `hello`")).await;
        return false;
    }
    send(
        writer,
        &Response::Welcome {
            protocol: PROTOCOL_VERSION,
            daemon_version: info.daemon_version.clone(),
            store_schema: info.store_schema,
        },
    )
    .await
}

async fn next_event(events: &mut Option<broadcast::Receiver<Event>>) -> Option<Event> {
    let Some(receiver) = events else {
        return std::future::pending().await;
    };
    match receiver.recv().await {
        Ok(event) => Some(event),
        // The subscriber missed events: tell it to re-read everything.
        Err(broadcast::error::RecvError::Lagged(_)) => Some(Event::StoreChanged {
            datasets: Dataset::ALL.to_vec(),
        }),
        Err(broadcast::error::RecvError::Closed) => {
            *events = None;
            None
        }
    }
}

async fn connection<H: Handler>(
    stream: ControlStream,
    handler: Arc<H>,
    info: ServerInfo,
    bus: EventBus,
    mut shutdown: Shutdown,
) {
    let (read, write) = stream.into_split();
    let mut reader = LineReader::new(read);
    let mut writer = LineWriter::new(write);
    if !handshake(&mut reader, &mut writer, &info).await {
        return;
    }
    let mut events: Option<broadcast::Receiver<Event>> = None;
    loop {
        tokio::select! {
            () = wait_shutdown(&mut shutdown) => {
                send(&mut writer, &Response::Error {
                    kind: ErrorKind::ShuttingDown,
                    message: "the daemon is shutting down".to_owned(),
                }).await;
                return;
            }
            event = next_event(&mut events) => {
                if let Some(event) = event
                    && !send(&mut writer, &Response::Event { event }).await
                {
                    return;
                }
            }
            line = reader.read_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => return,
                    Err(error) => {
                        tracing::debug!(%error, "control read failed");
                        return;
                    }
                };
                let response = match serde_json::from_str::<Request>(&line) {
                    Ok(Request::Hello { .. }) => bad_request("already greeted"),
                    Ok(Request::Subscribe) => {
                        if events.is_none() {
                            events = Some(bus.subscribe());
                        }
                        Response::Subscribed
                    }
                    Ok(request) => handler.handle(request).await,
                    Err(_) => bad_request("not a valid request"),
                };
                if !send(&mut writer, &response).await {
                    return;
                }
            }
        }
    }
}
