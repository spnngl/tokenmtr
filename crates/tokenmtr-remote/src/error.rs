//! The one error type of the HTTP layer. It never carries request headers or bodies (D55).

/// Coarse class of a transport failure. Deliberately drops the underlying `reqwest` error, whose
/// text can embed URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NetworkKind {
    /// The request or connect timeout elapsed.
    #[error("timed out")]
    Timeout,
    /// DNS, TCP or TLS setup failed.
    #[error("cannot connect")]
    Connect,
    /// The connection broke while reading the body.
    #[error("broken response body")]
    Body,
    /// Any other transport failure.
    #[error("request failed")]
    Other,
}

impl NetworkKind {
    pub(crate) fn of(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() {
            Self::Connect
        } else if error.is_body() || error.is_decode() {
            Self::Body
        } else {
            Self::Other
        }
    }
}

/// Failure of one HTTP exchange.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// The shared HTTP client could not be built.
    #[error("cannot build the HTTP client: {0}")]
    ClientBuild(#[source] reqwest::Error),
    /// An endpoint base URL does not parse.
    #[error("invalid endpoint URL {0:?}")]
    BadUrl(String),
    /// A request URL is not on the allow-list: an [`crate::ALLOWED_HOSTS`] host over HTTPS, or a
    /// loopback host (D13). Carries the host only.
    #[error("host {0:?} is not allowed")]
    HostNotAllowed(String),
    /// Transport failure.
    #[error("network error: {0}")]
    Network(NetworkKind),
    /// The server answered with a status the caller does not handle.
    #[error("unexpected HTTP status {0}")]
    Status(u16),
    /// HTTP 304 to a request that has no cached copy to fall back on.
    #[error("not modified, and there is no cached copy")]
    NotModified,
    /// The body is larger than the endpoint's cap.
    #[error("response body exceeds {limit} bytes")]
    TooLarge {
        /// The cap that was exceeded.
        limit: usize,
    },
    /// The body does not have the expected shape.
    #[error("malformed response: {0}")]
    Malformed(&'static str),
}

/// Where and how a JSON document failed to parse. Keeps the category and position only: serde
/// messages quote the offending value, which must not be echoed (D55).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JsonPosition {
    category: serde_json::error::Category,
    line: usize,
    column: usize,
}

impl From<serde_json::Error> for JsonPosition {
    fn from(error: serde_json::Error) -> Self {
        Self {
            category: error.classify(),
            line: error.line(),
            column: error.column(),
        }
    }
}

impl std::fmt::Display for JsonPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} at line {}, column {}",
            self.category, self.line, self.column
        )
    }
}
