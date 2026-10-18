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
    /// Transport failure.
    #[error("network error: {0}")]
    Network(NetworkKind),
    /// The server answered with a status the caller does not handle.
    #[error("unexpected HTTP status {0}")]
    Status(u16),
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
