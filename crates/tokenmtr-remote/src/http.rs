//! The shared HTTP client, endpoint base URLs and conditional GET.

use std::net::IpAddr;
use std::time::Duration;

use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode, Url};

use crate::error::{NetworkKind, RemoteError};

/// `User-Agent` of every request: `<brand CLI name>/<workspace version>`, e.g. `tokenmtr/0.3.0`.
#[must_use]
pub fn user_agent() -> String {
    format!(
        "{}/{}",
        tokenmtr_core::brand::CLI_NAME,
        env!("CARGO_PKG_VERSION")
    )
}

/// The only production hosts.
pub const ALLOWED_HOSTS: [&str; 3] = [
    "api.anthropic.com",
    "www.ecb.europa.eu",
    "raw.githubusercontent.com",
];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest silence between two reads of one response. It bounds stalls without capping how long
/// a large, steadily flowing body may take.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Whole-exchange cap of the small API calls (quota, profile).
pub(crate) const API_TIMEOUT: Duration = Duration::from_secs(30);
/// Whole-exchange backstop of the multi-megabyte downloads (ECB history, `LiteLLM` list).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);
const MAX_REDIRECTS: usize = 5;

/// Builds the one `reqwest` client every endpoint shares: rustls, connect and idle-read
/// timeouts, the tokenmtr user-agent, and redirects that stay on the original host and on the
/// allow-list ([`may_follow`]).
///
/// # Errors
/// [`RemoteError::ClientBuild`] if the TLS backend cannot initialise.
pub fn build_client() -> Result<Client, RemoteError> {
    base_builder()
        .redirect(Policy::custom(|attempt| {
            if may_follow(attempt.previous(), attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(RemoteError::ClientBuild)
}

/// The client of remote-write pushes. It follows no redirect: the URL is the user's, and a
/// receiver that redirects a push has answered with a refusal.
///
/// # Errors
/// [`RemoteError::ClientBuild`] if the TLS backend cannot initialise.
pub fn build_push_client() -> Result<Client, RemoteError> {
    base_builder()
        .redirect(Policy::none())
        .build()
        .map_err(RemoteError::ClientBuild)
}

fn base_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .user_agent(user_agent())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
}

/// Whether a request may go to `url`: an [`ALLOWED_HOSTS`] host over HTTPS, or a loopback
/// host over any scheme (the mock servers of tests; loopback cannot leak anything off the
/// machine).
fn is_allowed(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    is_loopback_host(host) || (url.scheme() == "https" && ALLOWED_HOSTS.contains(&host))
}

/// Whether `host` is `localhost` or a loopback IP address, bracketed or not.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether to follow a redirect to `next` after `previous` hops: same host as the last hop, no
/// HTTPS-to-HTTP downgrade, allowed by [`is_allowed`], and fewer than [`MAX_REDIRECTS`] hops.
fn may_follow(previous: &[Url], next: &Url) -> bool {
    previous.last().is_some_and(|prev| {
        prev.host_str() == next.host_str() && (prev.scheme() != "https" || next.scheme() == "https")
    }) && is_allowed(next)
        && previous.len() < MAX_REDIRECTS
}

/// Base URLs of the three remote hosts. Tests point them at a mock server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Host of `/api/oauth/usage` and `/api/oauth/profile`.
    pub anthropic: Url,
    /// Host of the `eurofxref-*.xml` files.
    pub ecb: Url,
    /// Host of the `LiteLLM` price list.
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

/// Joins an absolute `path` onto `base`, replacing any path the base has. Every request URL is
/// built here, so this is where the allow-list ([`is_allowed`]) is enforced.
///
/// # Errors
/// [`RemoteError::BadUrl`] if the join fails, [`RemoteError::HostNotAllowed`] if the result is
/// not allowed.
pub(crate) fn join(base: &Url, path: &str) -> Result<Url, RemoteError> {
    let url = base
        .join(path)
        .map_err(|_| RemoteError::BadUrl(path.to_owned()))?;
    if is_allowed(&url) {
        Ok(url)
    } else {
        Err(RemoteError::HostNotAllowed(
            url.host_str().unwrap_or_default().to_owned(),
        ))
    }
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
    let mut request = client.get(url).timeout(DOWNLOAD_TIMEOUT);
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
        assert_eq!(
            user_agent(),
            concat!("tokenmtr/", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn join_replaces_the_base_path() -> Result<(), RemoteError> {
        let base = parse_base("http://127.0.0.1:1/ignored")?;
        assert_eq!(join(&base, "/api/x")?.path(), "/api/x");
        Ok(())
    }

    #[test]
    fn join_enforces_the_allow_list() -> Result<(), RemoteError> {
        for allowed in [
            "https://api.anthropic.com",
            "https://raw.githubusercontent.com",
            "http://localhost:8080",
            "http://[::1]:8080",
        ] {
            assert!(join(&parse_base(allowed)?, "/x").is_ok(), "{allowed}");
        }
        for refused in [
            "http://api.anthropic.com",
            "https://example.com",
            "https://api.anthropic.com.evil.test",
            "http://10.0.0.1",
        ] {
            assert!(
                matches!(
                    join(&parse_base(refused)?, "/x"),
                    Err(RemoteError::HostNotAllowed(_))
                ),
                "{refused}"
            );
        }
        Ok(())
    }

    #[test]
    fn redirects_stay_on_host_https_and_hop_limit() -> Result<(), RemoteError> {
        let url = |s: &str| parse_base(s);
        let ecb = url("https://www.ecb.europa.eu/a")?;
        assert!(may_follow(
            std::slice::from_ref(&ecb),
            &url("https://www.ecb.europa.eu/b")?
        ));
        assert!(!may_follow(
            std::slice::from_ref(&ecb),
            &url("http://www.ecb.europa.eu/b")?
        ));
        assert!(!may_follow(
            std::slice::from_ref(&ecb),
            &url("https://api.anthropic.com/b")?
        ));
        assert!(!may_follow(&[], &url("https://www.ecb.europa.eu/b")?));
        let hops = vec![ecb; MAX_REDIRECTS];
        assert!(!may_follow(&hops, &url("https://www.ecb.europa.eu/b")?));
        Ok(())
    }
}
