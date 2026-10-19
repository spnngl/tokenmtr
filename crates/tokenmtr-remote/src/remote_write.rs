//! Pushes one Prometheus Remote Write 1.0 request to a user-configured endpoint.
//!
//! The URL is the user's own, so it is outside [`crate::ALLOWED_HOSTS`]; [`Target::new`] admits
//! it under a separate rule: HTTPS, or a loopback host over any scheme. The push client never follows
//! a redirect (`build_push_client`), so a redirecting receiver answers with its 3xx, which
//! [`push`] reports like any other rejection.
//!
//! One call is one attempt. Back-off, batching and bookkeeping belong to the daemon; this module
//! only says whether a failure is worth repeating ([`PushError::Retry`]).

use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode, Url};
use secrecy::ExposeSecret as _;

use crate::credential::Credential;
use crate::error::{NetworkKind, RemoteError};
use crate::http::{API_TIMEOUT, is_loopback_host};

const PROTOBUF: &str = "application/x-protobuf";
const SNAPPY: &str = "snappy";
const VERSION_HEADER: &str = "x-prometheus-remote-write-version";
const REMOTE_WRITE_1: &str = "0.1.0";
const TENANT_HEADER: &str = "x-scope-orgid";

/// Where a push goes and how it identifies itself.
#[derive(Debug, Clone)]
pub struct Target {
    url: Url,
    tenant: Option<HeaderValue>,
    credential: Option<Credential>,
}

impl Target {
    /// A target for `url`, with an optional `X-Scope-OrgID` tenant and credential.
    ///
    /// # Errors
    ///
    /// [`RemoteError::HostNotAllowed`] unless `url` is HTTPS or on a loopback host;
    /// [`RemoteError::Malformed`] if `url` has embedded credentials or `tenant` is not a valid
    /// header value.
    pub fn new(
        url: Url,
        tenant: Option<&str>,
        credential: Option<Credential>,
    ) -> Result<Self, RemoteError> {
        let host = url.host_str().unwrap_or_default();
        if !(url.scheme() == "https" || (url.scheme() == "http" && is_loopback_host(host))) {
            return Err(RemoteError::HostNotAllowed(host.to_owned()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(RemoteError::Malformed("credentials in the URL"));
        }
        let tenant = tenant
            .map(|t| HeaderValue::from_str(t).map_err(|_| RemoteError::Malformed("tenant")))
            .transpose()?;
        Ok(Self {
            url,
            tenant,
            credential,
        })
    }
}

/// Why a push failed, and whether sending the same bytes again can help.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// A timeout, a transport failure, 408, 429 or a 5xx other than 501: try again later.
    #[error("{0}")]
    Retry(RemoteError),
    /// The receiver refused the request (any other status): resending cannot succeed.
    #[error("{0}")]
    Drop(RemoteError),
}

impl PushError {
    /// Whether the push is worth repeating.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Retry(_))
    }

    fn of_status(status: StatusCode) -> Self {
        let retry = matches!(status.as_u16(), 408 | 429 | 500 | 502..=599);
        let error = RemoteError::Status(status.as_u16());
        if retry {
            Self::Retry(error)
        } else {
            Self::Drop(error)
        }
    }
}

/// Sends `body` (a snappy-compressed `WriteRequest`, see `tokenmtr_metrics::Request::encode`).
///
/// # Errors
///
/// [`PushError::Retry`] or [`PushError::Drop`], carrying a status code or a transport kind and
/// nothing else: never headers, bodies or the URL.
pub async fn push(client: &Client, target: &Target, body: &[u8]) -> Result<(), PushError> {
    let mut request = client
        .post(target.url.clone())
        .timeout(API_TIMEOUT)
        .header(CONTENT_TYPE, PROTOBUF)
        .header(CONTENT_ENCODING, SNAPPY)
        .header(HeaderName::from_static(VERSION_HEADER), REMOTE_WRITE_1)
        .body(body.to_vec());
    if let Some(tenant) = &target.tenant {
        request = request.header(HeaderName::from_static(TENANT_HEADER), tenant.clone());
    }
    request = match &target.credential {
        Some(Credential::Basic { username, password }) => {
            request.basic_auth(username, Some(password.expose_secret()))
        }
        Some(Credential::Bearer(token)) => request.bearer_auth(token.expose_secret()),
        None => request,
    };
    let response = request
        .send()
        .await
        .map_err(|e| PushError::Retry(RemoteError::Network(NetworkKind::of(&e))))?;
    let status = response.status();
    tracing::debug!(status = status.as_u16(), "remote write answered");
    if status.is_success() {
        Ok(())
    } else {
        Err(PushError::of_status(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_classified_by_whether_a_resend_can_help() {
        let retryable = |code: u16| {
            PushError::of_status(StatusCode::from_u16(code).unwrap_or(StatusCode::OK))
                .is_retryable()
        };
        for code in [408, 429, 500, 502, 503, 504, 599] {
            assert!(retryable(code), "{code}");
        }
        for code in [301, 400, 401, 403, 404, 413, 415, 501] {
            assert!(!retryable(code), "{code}");
        }
    }
}
