//! Errors of the control client.

use std::io;

use tokenmtr_platform::FrameError;

use crate::protocol::{ErrorKind, PROTOCOL_VERSION};

/// Why a client call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Nobody is listening on the socket.
    #[error("cannot connect to the daemon: {0}")]
    Connect(#[source] io::Error),
    /// The transport broke or the daemon sent a bad frame.
    #[error("control transport: {0}")]
    Frame(#[from] FrameError),
    /// A frame is not a message of this protocol.
    #[error("malformed control message: {0}")]
    Json(#[from] serde_json::Error),
    /// The daemon speaks another protocol version and sent no data.
    #[error(
        "daemon {daemon_version} speaks control protocol {daemon_protocol} (store schema {store_schema}), \
         this client speaks {PROTOCOL_VERSION}: {reason}"
    )]
    Incompatible {
        /// The daemon's protocol version.
        daemon_protocol: u32,
        /// The daemon's package version.
        daemon_version: String,
        /// The daemon's store schema version.
        store_schema: u32,
        /// The daemon's explanation.
        reason: String,
    },
    /// The daemon closed the connection.
    #[error("the daemon closed the connection")]
    Closed,
    /// The daemon answered with a message that does not fit the request.
    #[error("unexpected `{0}` message from the daemon")]
    Unexpected(&'static str),
    /// The daemon understood the request and refused or failed it.
    #[error("daemon error ({kind:?}): {message}")]
    Remote {
        /// Machine-readable class.
        kind: ErrorKind,
        /// The daemon's explanation.
        message: String,
    },
}
