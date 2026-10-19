//! The error every command returns: a machine-readable kind plus the message to show.
//!
//! Serialised as `{ "kind": "...", "message": "..." }`; the UI's `commandErrorSchema` parses it.

use serde::Serialize;
use tokenmtr_app::AppError;
use tokenmtr_control::ClientError;
use tokenmtr_platform::ServiceError;

use crate::daemon::SidecarError;

/// Class of a [`CommandError`]; the UI branches on it (for example offline vs. incompatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The request does not name anything valid (unknown agent, bad range, bad path).
    InvalidRequest,
    /// No daemon answers on the control socket.
    DaemonOffline,
    /// The daemon speaks another protocol version.
    DaemonIncompatible,
    /// The action needs the daemon stopped but it runs.
    DaemonRunning,
    /// The daemon refused or failed the request.
    Remote,
    /// The store cannot be read.
    Store,
    /// The config is invalid or cannot be edited.
    Config,
    /// A file operation failed.
    Io,
    /// The service manager (launchd, systemd) refused an operation.
    Service,
    /// The app runs from a disk image or a translocated copy: it must be moved to Applications
    /// before the background service can point at it.
    AppNotInstalled,
    /// Anything else, including a panicked worker.
    Internal,
}

impl ErrorKind {
    /// Every kind with its wire name, for the generated `bindings.ts`.
    pub const ALL: [(Self, &'static str); 11] = [
        (Self::InvalidRequest, "invalid_request"),
        (Self::DaemonOffline, "daemon_offline"),
        (Self::DaemonIncompatible, "daemon_incompatible"),
        (Self::DaemonRunning, "daemon_running"),
        (Self::Remote, "remote"),
        (Self::Store, "store"),
        (Self::Config, "config"),
        (Self::Io, "io"),
        (Self::Service, "service"),
        (Self::AppNotInstalled, "app_not_installed"),
        (Self::Internal, "internal"),
    ];
}

/// A failed command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct CommandError {
    /// What went wrong, as a class.
    pub kind: ErrorKind,
    /// A sentence for the user.
    pub message: String,
}

impl CommandError {
    /// An error of `kind` with `message`.
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// [`ErrorKind::InvalidRequest`].
    #[must_use]
    pub fn invalid(message: impl std::fmt::Display) -> Self {
        Self::new(ErrorKind::InvalidRequest, message.to_string())
    }

    /// [`ErrorKind::Internal`].
    #[must_use]
    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::new(ErrorKind::Internal, message.to_string())
    }
}

impl From<AppError> for CommandError {
    fn from(error: AppError) -> Self {
        let kind = match &error {
            AppError::Config(_) | AppError::AgentSettings { .. } => ErrorKind::Config,
            AppError::Store(_) | AppError::Query(_) | AppError::Ingest(_) => ErrorKind::Store,
            AppError::Filter(_)
            | AppError::Report(_)
            | AppError::Render(_)
            | AppError::InvalidRequest(_) => ErrorKind::InvalidRequest,
            AppError::Io { .. } => ErrorKind::Io,
            AppError::DaemonRunning { .. } => ErrorKind::DaemonRunning,
            AppError::Service(service) => service_kind(service),
        };
        Self::new(kind, error.to_string())
    }
}

const fn service_kind(error: &ServiceError) -> ErrorKind {
    match error {
        ServiceError::TransientBinary(_) => ErrorKind::AppNotInstalled,
        _ => ErrorKind::Service,
    }
}

impl From<ClientError> for CommandError {
    fn from(error: ClientError) -> Self {
        let kind = match &error {
            // A daemon that does not answer in time is treated like a missing one: restarting
            // it is the remedy for both.
            ClientError::Connect(_) | ClientError::Closed | ClientError::Timeout(_) => {
                ErrorKind::DaemonOffline
            }
            ClientError::Incompatible { .. } => ErrorKind::DaemonIncompatible,
            ClientError::Remote { .. } => ErrorKind::Remote,
            ClientError::Frame(_) | ClientError::Json(_) | ClientError::Unexpected(_) => {
                ErrorKind::Internal
            }
        };
        Self::new(kind, error.to_string())
    }
}

impl From<tokenmtr_query::QueryError> for CommandError {
    fn from(error: tokenmtr_query::QueryError) -> Self {
        Self::new(ErrorKind::Store, error.to_string())
    }
}

impl From<tokenmtr_query::FilterError> for CommandError {
    fn from(error: tokenmtr_query::FilterError) -> Self {
        Self::invalid(error)
    }
}

impl From<ServiceError> for CommandError {
    fn from(error: ServiceError) -> Self {
        Self::new(service_kind(&error), error.to_string())
    }
}

impl From<SidecarError> for CommandError {
    fn from(error: SidecarError) -> Self {
        let kind = match &error {
            SidecarError::Service(service) => service_kind(service),
            SidecarError::Missing(_) => ErrorKind::Service,
            SidecarError::Executable(_) | SidecarError::NotUtf8(_) => ErrorKind::Internal,
        };
        Self::new(kind, error.to_string())
    }
}

impl From<tokenmtr_store::StoreError> for CommandError {
    fn from(error: tokenmtr_store::StoreError) -> Self {
        Self::new(ErrorKind::Store, error.to_string())
    }
}

impl From<tauri::Error> for CommandError {
    fn from(error: tauri::Error) -> Self {
        Self::internal(error)
    }
}

impl From<tokio::task::JoinError> for CommandError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::internal(format!("worker failed: {error}"))
    }
}

impl From<std::io::Error> for CommandError {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_as_kind_and_message() -> Result<(), serde_json::Error> {
        let error = CommandError::invalid("bad agent");
        assert_eq!(
            serde_json::to_value(&error)?,
            serde_json::json!({ "kind": "invalid_request", "message": "bad agent" })
        );
        Ok(())
    }

    #[test]
    fn listed_names_are_the_wire_names() -> Result<(), serde_json::Error> {
        for (kind, name) in ErrorKind::ALL {
            assert_eq!(serde_json::to_value(kind)?, serde_json::json!(name));
        }
        Ok(())
    }

    #[test]
    fn app_errors_map_to_kinds() {
        let error: CommandError = AppError::InvalidRequest("nope".to_owned()).into();
        assert_eq!(error.kind, ErrorKind::InvalidRequest);
        let error: CommandError = ClientError::Closed.into();
        assert_eq!(error.kind, ErrorKind::DaemonOffline);
        let error: CommandError =
            ServiceError::TransientBinary("/Volumes/T/tokenmtr".into()).into();
        assert_eq!(error.kind, ErrorKind::AppNotInstalled);
    }
}
