//! The shared HTTP client, endpoint base URLs and conditional GET (D13).

use std::time::Duration;

use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode, Url};

use crate::error::{NetworkKind, RemoteError};

/// `User-Agent` of every request: `tokenmtr/<workspace version>`.
pub const USER_AGENT: &str = concat!("tokenmtr/", env!("CARGO_PKG_VERSION"));

/// The only production hosts (D13).
pub const ALLOWED_HOSTS: [&str; 3] = [
    "api.anthropic.com",
    "www.ecb.europa.eu",
    "raw.githubusercontent.com",
];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

/// Builds the one `reqwest` client every endpoint shares: rustls, timeouts, the tokenmtr
/// user-agent, and redirects that never leave the original host.
///
/// # Errors
/// [`RemoteError::ClientBuild`] if the TLS backend cannot initialise.
pub fn build_client() -> Result<Client, RemoteError> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::custom(|attempt| {
            let same_host = attempt
                .previous()
                .last()
                .is_some_and(|prev| prev.host_str() == attempt.url().host_str());
            if same_host && attempt.previous().len() < MAX_REDIRECTS {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(RemoteError::ClientBuild)
}

/// Base URLs of the three remote hosts. Tests point them at a mock server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Host of `/api/oauth/usage` and `/api/oauth/profile` (D26).
    pub anthropic: Url,
    /// Host of the `eurofxref-*.xml` files (D27).
    pub ecb: Url,
    /// Host of the `LiteLLM` price list (D11).
    pub github_raw: Url,
}

impl Endpoints {
    /// The real hosts of [`ALLOWED_HOSTS`].
    ///
    /// # Errors
    /// [`RemoteError::BadUrl`] never in practice; a test pins the constants.
    pub fn production() -> Result<Self, RemoteError> {
        Ok(Self {
            anthropic: parse_base("https://api.anthropic.com")?,
            ecb: parse_base("https://www.ecb.europa.eu")?,
            github_raw: parse_base("https://raw.githubusercontent.com")?,
        })
    }

    /// All three endpoints at one base URL, e.g. a mock server.
    ///
    /// # Errors
    /// [`RemoteError::BadUrl`] if `base` is not a URL.
    pub fn at(base: &str) -> Result<Self, RemoteError> {
        let url = parse_base(base)?;
        Ok(Self {
            anthropic: url.clone(),
            ecb: url.clone(),
            github_raw: url,
        })
    }
}

fn parse_base(base: &str) -> Result<Url, RemoteError> {
    Url::parse(base).map_err(|_| RemoteError::BadUrl(base.to_owned()))
}

/// Joins an absolute `path` onto `base`, replacing any path the base has.
pub(crate) fn join(base: &Url, path: &str) -> Result<Url, RemoteError> {
    base.join(path)
        .map_err(|_| RemoteError::BadUrl(path.to_owned()))
}

/// Cache validators a server returned, replayed on the next request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    /// `ETag`, sent back as `If-None-Match`.
    pub etag: Option<String>,
    /// `Last-Modified`, sent back as `If-Modified-Since`.
    pub last_modified: Option<String>,
}

impl Validators {
    /// Validators holding only an `ETag`.
    #[must_use]
    pub fn etag(etag: Option<&str>) -> Self {
        Self {
            etag: etag.map(str::to_owned),
            last_modified: None,
        }
    }

    fn of(response: &Response) -> Self {
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        Self {
            etag: header(ETAG),
            last_modified: header(LAST_MODIFIED),
        }
    }
}

/// Result of a conditional GET.
#[derive(Debug)]
pub(crate) enum Fetched {
    /// HTTP 304: the caller's copy is current.
    NotModified,
    /// HTTP 200 with the body and the validators to store.
    Body {
        body: Vec<u8>,
        validators: Validators,
    },
}

/// GET `url` with `validators`, capping the body at `limit` bytes.
pub(crate) async fn get_conditional(
    client: &Client,
    url: Url,
    validators: &Validators,
    limit: usize,
) -> Result<Fetched, RemoteError> {
    let mut request = client.get(url);
    if let Some(etag) = &validators.etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    if let Some(since) = &validators.last_modified {
        request = request.header(IF_MODIFIED_SINCE, since);
    }
    let response = request
        .send()
        .await
        .map_err(|e| RemoteError::Network(NetworkKind::of(&e)))?;
    let status = response.status();
    tracing::debug!(status = status.as_u16(), "conditional GET answered");
    if status == StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !status.is_success() {
        return Err(RemoteError::Status(status.as_u16()));
    }
    let validators = Validators::of(&response);
    let body = read_capped(response, limit).await?;
    Ok(Fetched::Body { body, validators })
}

/// Reads a response body, failing once it exceeds `limit` bytes.
pub(crate) async fn read_capped(
    mut response: Response,
    limit: usize,
) -> Result<Vec<u8>, RemoteError> {
    let too_large = RemoteError::TooLarge { limit };
    if response
        .content_length()
        .is_some_and(|len| usize::try_from(len).map_or(true, |len| len > limit))
    {
        return Err(too_large);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| RemoteError::Network(NetworkKind::of(&e)))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(too_large);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_endpoints_are_exactly_the_allow_list() -> Result<(), RemoteError> {
        let endpoints = Endpoints::production()?;
        let hosts = [&endpoints.anthropic, &endpoints.ecb, &endpoints.github_raw]
            .map(|url| url.host_str().map(str::to_owned));
        assert_eq!(hosts, ALLOWED_HOSTS.map(|h| Some(h.to_owned())));
        Ok(())
    }

    #[test]
    fn user_agent_carries_the_workspace_version() {
        assert_eq!(USER_AGENT, concat!("tokenmtr/", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn join_replaces_the_base_path() -> Result<(), RemoteError> {
        let base = parse_base("http://127.0.0.1:1/ignored")?;
        assert_eq!(join(&base, "/api/x")?.path(), "/api/x");
        Ok(())
    }
}
