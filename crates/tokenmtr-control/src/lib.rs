//! The daemon's control channel (D24, D52): protocol v1 types, an async [`Client`] and a
//! [`Server`] building block.
//!
//! Messages are JSON objects, one per line, over the platform's control transport (a Unix domain
//! socket). The first message of a connection is [`Request::Hello`]; the daemon answers
//! [`Response::Welcome`], or [`Response::Incompatible`] with no data when the client's protocol
//! version differs from [`PROTOCOL_VERSION`]. See `docs/control-protocol.md` for the message
//! table and the versioning rules.
//!
//! The crate knows nothing about the store or the scheduler: the daemon plugs in a [`Handler`]
//! for the requests that need state and publishes [`Event`]s on an [`EventBus`].

mod client;
mod error;
mod protocol;
mod server;

pub use client::{Client, DaemonInfo, Subscription};
pub use error::ClientError;
pub use protocol::{
    AgentScanSummary, BackupSummary, ClientInfo, DaemonStatus, Dataset, ErrorKind, Event,
    ExportSummary, IssueKind, IssueList, IssueSummary, PROTOCOL_VERSION, QuotaProfileSummary,
    QuotaSummary, Request, Response, ScanSummary, SinkExportSummary, SinkSummary, Task,
};
pub use server::{EventBus, Handler, Server, ServerInfo, Shutdown, wait_shutdown};

#[cfg(test)]
mod tests;
