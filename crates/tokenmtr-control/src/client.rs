//! The control client: connect, handshake, request/response, event stream.
//!
//! Every step except waiting for pushed events is bounded, so a wedged daemon makes callers
//! fail with [`ClientError::Timeout`] instead of hanging.

use std::time::Duration;

use tokenmtr_core::{AgentId, SinkId};
use tokenmtr_platform::{ControlStream, ControlTransport, LineReader, LineWriter};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::error::ClientError;
use crate::protocol::{
    BackupSummary, ClientInfo, DaemonStatus, Event, ExportSummary, IssueList, PROTOCOL_VERSION,
    QuotaSummary, RepriceSummary, Request, Response, ScanSummary,
};

/// Bound on connecting plus the `Hello` handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound on requests that only read daemon state or flip a flag.
const QUICK_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on requests that scan, export, back up or reach the network.
const WORK_TIMEOUT: Duration = Duration::from_secs(300);

/// How long `request` may take before the daemon counts as unresponsive.
const fn request_timeout(request: &Request) -> Duration {
    match request {
        Request::ScanNow { .. }
        | Request::ExportNow { .. }
        | Request::BackupNow
        | Request::Reprice { .. }
        | Request::RefreshQuota
        | Request::ConnectKeychain => WORK_TIMEOUT,
        Request::Hello { .. }
        | Request::Status
        | Request::Reload
        | Request::Issues
        | Request::Subscribe
        | Request::Shutdown => QUICK_TIMEOUT,
    }
}

/// Runs `step`, failing with [`ClientError::Timeout`] after `limit`.
async fn within<T>(
    limit: Duration,
    step: impl Future<Output = Result<T, ClientError>>,
) -> Result<T, ClientError> {
    tokio::time::timeout(limit, step)
        .await
        .map_err(|_| ClientError::Timeout(limit))?
}

/// What the daemon reported in `Welcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonInfo {
    /// The daemon's package version.
    pub daemon_version: String,
    /// The store schema version the daemon runs.
    pub store_schema: u32,
}

/// A handshaken connection to the daemon.
#[derive(Debug)]
pub struct Client {
    reader: LineReader<OwnedReadHalf>,
    writer: LineWriter<OwnedWriteHalf>,
    daemon: DaemonInfo,
    /// A timed-out request may still be answered later, so the stream is out of step.
    timed_out: bool,
}

impl Client {
    /// Connects through `transport` and completes the handshake, within 2 s.
    ///
    /// # Errors
    /// [`ClientError::Connect`] when no daemon listens, [`ClientError::Incompatible`] when the
    /// protocol versions differ (the daemon sends no data then), [`ClientError::Timeout`], or a
    /// transport error.
    pub async fn connect(
        transport: &ControlTransport,
        client: ClientInfo,
    ) -> Result<Self, ClientError> {
        within(CONNECT_TIMEOUT, async {
            let stream = transport.connect().await.map_err(ClientError::Connect)?;
            Self::handshake(stream, client).await
        })
        .await
    }

    /// Handshakes over an already connected stream, within 2 s.
    ///
    /// # Errors
    /// As [`Client::connect`], except for the connect failure.
    pub async fn over(stream: ControlStream, client: ClientInfo) -> Result<Self, ClientError> {
        within(CONNECT_TIMEOUT, Self::handshake(stream, client)).await
    }

    async fn handshake(stream: ControlStream, client: ClientInfo) -> Result<Self, ClientError> {
        let (read, write) = stream.into_split();
        let mut reader = LineReader::new(read);
        let mut writer = LineWriter::new(write);
        let hello = Request::Hello {
            protocol: PROTOCOL_VERSION,
            client,
        };
        writer.write_line(&serde_json::to_string(&hello)?).await?;
        match read_response(&mut reader).await? {
            Response::Welcome {
                daemon_version,
                store_schema,
                ..
            } => Ok(Self {
                reader,
                writer,
                daemon: DaemonInfo {
                    daemon_version,
                    store_schema,
                },
                timed_out: false,
            }),
            Response::Incompatible {
                daemon_protocol,
                daemon_version,
                store_schema,
                reason,
            } => Err(ClientError::Incompatible {
                daemon_protocol,
                daemon_version,
                store_schema,
                reason,
            }),
            Response::Error { kind, message } => Err(ClientError::Remote { kind, message }),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// What the daemon said about itself.
    #[must_use]
    pub fn daemon(&self) -> &DaemonInfo {
        &self.daemon
    }

    /// Sends one request and returns its response. A [`Response::Error`] becomes
    /// [`ClientError::Remote`].
    ///
    /// State reads time out after 10 s; scans, exports, backups and quota refreshes after
    /// 5 min. After a timeout every further request fails with [`ClientError::Closed`].
    ///
    /// # Errors
    /// Transport failures, a closed connection, a timeout, or a daemon-side error.
    pub async fn request(&mut self, request: &Request) -> Result<Response, ClientError> {
        if self.timed_out {
            return Err(ClientError::Closed);
        }
        let line = serde_json::to_string(request)?;
        let (reader, writer) = (&mut self.reader, &mut self.writer);
        let answer = within(request_timeout(request), async {
            writer.write_line(&line).await?;
            read_response(reader).await
        })
        .await;
        self.timed_out = matches!(answer, Err(ClientError::Timeout(_)));
        match answer? {
            Response::Error { kind, message } => Err(ClientError::Remote { kind, message }),
            response => Ok(response),
        }
    }

    /// Daemon health.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn status(&mut self) -> Result<DaemonStatus, ClientError> {
        match self.request(&Request::Status).await? {
            Response::Status(status) => Ok(status),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Scans now.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn scan_now(&mut self, agent: Option<AgentId>) -> Result<ScanSummary, ClientError> {
        match self.request(&Request::ScanNow { agent }).await? {
            Response::Scanned(summary) => Ok(summary),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Exports now.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn export_now(&mut self, sink: Option<SinkId>) -> Result<ExportSummary, ClientError> {
        match self.request(&Request::ExportNow { sink }).await? {
            Response::Exported(summary) => Ok(summary),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Backs up now.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn backup_now(&mut self) -> Result<BackupSummary, ClientError> {
        match self.request(&Request::BackupNow).await? {
            Response::BackedUp(summary) => Ok(summary),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Re-estimates stored events with the active pricing snapshot: the unpriced ones, or every
    /// one with `all`.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn reprice(&mut self, all: bool) -> Result<RepriceSummary, ClientError> {
        match self.request(&Request::Reprice { all }).await? {
            Response::Repriced(summary) => Ok(summary),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Polls quota now.
    ///
    /// # Errors
    /// As [`Client::request`]; `Disabled` when `[quota]` is off.
    pub async fn refresh_quota(&mut self) -> Result<QuotaSummary, ClientError> {
        match self.request(&Request::RefreshQuota).await? {
            Response::QuotaRefreshed(summary) => Ok(summary),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Reads the Claude credentials from the Keychain once with the consent prompt allowed,
    /// then polls quota.
    ///
    /// # Errors
    /// As [`Client::request`]; `Disabled` when `[quota]` is off.
    pub async fn connect_keychain(&mut self) -> Result<QuotaSummary, ClientError> {
        match self.request(&Request::ConnectKeychain).await? {
            Response::QuotaRefreshed(summary) => Ok(summary),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Reloads the config now.
    ///
    /// # Errors
    /// As [`Client::request`]; `InvalidConfig` when the file is rejected.
    pub async fn reload(&mut self) -> Result<(), ClientError> {
        match self.request(&Request::Reload).await? {
            Response::Reloaded => Ok(()),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Open problems.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn issues(&mut self) -> Result<IssueList, ClientError> {
        match self.request(&Request::Issues).await? {
            Response::Issues(list) => Ok(list),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Asks the daemon to stop gracefully. The daemon answers first; the control socket
    /// disappears once it has stopped.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn shutdown(&mut self) -> Result<(), ClientError> {
        match self.request(&Request::Shutdown).await? {
            Response::ShuttingDown => Ok(()),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }

    /// Turns the connection into an event stream.
    ///
    /// # Errors
    /// As [`Client::request`].
    pub async fn subscribe(mut self) -> Result<Subscription, ClientError> {
        match self.request(&Request::Subscribe).await? {
            Response::Subscribed => Ok(Subscription { client: self }),
            other => Err(ClientError::Unexpected(type_name(&other))),
        }
    }
}

/// A connection receiving pushed [`Event`]s. A closed connection means the daemon is gone.
#[derive(Debug)]
pub struct Subscription {
    client: Client,
}

impl Subscription {
    /// The next event; `None` when the daemon closed the connection.
    ///
    /// Cancel-safe: dropping the future loses no event.
    ///
    /// # Errors
    /// Transport failures or a message that is not an event.
    pub async fn next(&mut self) -> Result<Option<Event>, ClientError> {
        loop {
            let Some(line) = self.client.reader.read_line().await? else {
                return Ok(None);
            };
            match serde_json::from_str::<Response>(&line)? {
                Response::Event { event } => return Ok(Some(event)),
                // A shutdown notice is followed by the close.
                Response::Error { .. } => {}
                other => return Err(ClientError::Unexpected(type_name(&other))),
            }
        }
    }
}

async fn read_response(reader: &mut LineReader<OwnedReadHalf>) -> Result<Response, ClientError> {
    let line = reader.read_line().await?.ok_or(ClientError::Closed)?;
    Ok(serde_json::from_str(&line)?)
}

const fn type_name(response: &Response) -> &'static str {
    match response {
        Response::Welcome { .. } => "welcome",
        Response::Incompatible { .. } => "incompatible",
        Response::Status(_) => "status",
        Response::Scanned(_) => "scanned",
        Response::Exported(_) => "exported",
        Response::BackedUp(_) => "backed_up",
        Response::Repriced(_) => "repriced",
        Response::QuotaRefreshed(_) => "quota_refreshed",
        Response::Reloaded => "reloaded",
        Response::Issues(_) => "issues",
        Response::ShuttingDown => "shutting_down",
        Response::Subscribed => "subscribed",
        Response::Event { .. } => "event",
        Response::Error { .. } => "error",
    }
}
