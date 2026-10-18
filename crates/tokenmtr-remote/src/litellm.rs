//! Daily refresh of the `LiteLLM` price list (D11).

use reqwest::{Client, Url};
use tokenmtr_core::{PricingCatalog, PricingError};

use crate::error::RemoteError;
use crate::http::{Fetched, Validators, get_conditional, join};

const CATALOG_PATH: &str = "/BerriAI/litellm/main/model_prices_and_context_window.json";
/// The vendored file is ~2 MB.
const BODY_LIMIT: usize = 64 << 20;

/// The result of [`LitellmRefresh::refresh`].
#[derive(Debug)]
pub enum RefreshOutcome {
    /// HTTP 304: the stored snapshot is current.
    NotModified,
    /// A body that parsed. The caller stores `body` as the new snapshot (its version is
    /// `catalog.version()`) and remembers `etag`.
    New {
        /// The parsed catalog.
        catalog: PricingCatalog,
        /// The exact response bytes, to persist.
        body: Vec<u8>,
        /// The response's `ETag`, to send as `If-None-Match` next time.
        etag: Option<String>,
    },
    /// A body that failed [`PricingCatalog::parse`]. The caller keeps its previous snapshot.
    Rejected(PricingError),
}

/// Validates a downloaded body the same way [`LitellmRefresh`] does.
///
/// # Errors
/// [`PricingError`] when the body is not a usable `LiteLLM` catalog.
pub fn parse_catalog(body: &[u8]) -> Result<PricingCatalog, PricingError> {
    PricingCatalog::parse(body)
}

/// `LiteLLM` price-list client.
#[derive(Debug, Clone)]
pub struct LitellmRefresh {
    client: Client,
    base: Url,
}

impl LitellmRefresh {
    /// Builds the client over a shared HTTP client and the `raw.githubusercontent.com` base URL
    /// ([`crate::Endpoints::github_raw`]).
    #[must_use]
    pub fn new(client: Client, base: Url) -> Self {
        Self { client, base }
    }

    /// Conditional GET of the price list, sending `etag` (from the last [`RefreshOutcome::New`])
    /// as `If-None-Match`. Parsing is CPU-bound (milliseconds); callers on a latency-sensitive
    /// runtime may want `spawn_blocking`.
    ///
    /// # Errors
    /// [`RemoteError`] on transport failure or a status other than 200 and 304. A bad body is
    /// not an error: it is [`RefreshOutcome::Rejected`].
    pub async fn refresh(&self, etag: Option<&str>) -> Result<RefreshOutcome, RemoteError> {
        let url = join(&self.base, CATALOG_PATH)?;
        let fetched =
            get_conditional(&self.client, url, &Validators::etag(etag), BODY_LIMIT).await?;
        Ok(match fetched {
            Fetched::NotModified => RefreshOutcome::NotModified,
            Fetched::Body { body, validators } => match parse_catalog(&body) {
                Ok(catalog) => RefreshOutcome::New {
                    catalog,
                    body,
                    etag: validators.etag,
                },
                Err(reason) => {
                    tracing::warn!(%reason, "rejecting LiteLLM snapshot");
                    RefreshOutcome::Rejected(reason)
                }
            },
        })
    }
}
