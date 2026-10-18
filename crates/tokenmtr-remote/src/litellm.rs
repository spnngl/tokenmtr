//! Daily refresh of the `LiteLLM` price list.

use reqwest::{Client, Url};
use tokenmtr_core::pricing::carry_forward;
use tokenmtr_core::{PricingCatalog, PricingError};

use crate::error::RemoteError;
use crate::http::{Fetched, Validators, get_conditional, join};

const CATALOG_PATH: &str = "/BerriAI/litellm/main/model_prices_and_context_window.json";
/// The vendored file is ~3 MB; the limit leaves room for growth.
const BODY_LIMIT: usize = 64 << 20;

/// The result of [`LitellmRefresh::refresh`].
#[derive(Debug)]
pub enum RefreshOutcome {
    /// HTTP 304: the stored snapshot is current.
    NotModified,
    /// A body that passed [`parse_catalog`]. The caller stores `body` as the new snapshot (its
    /// version is `catalog.version()`) and remembers `etag`.
    New {
        /// The parsed catalog.
        catalog: PricingCatalog,
        /// The bytes to persist: the download, plus the active snapshot's models it dropped.
        body: Vec<u8>,
        /// The response's `ETag`, to send as `If-None-Match` next time.
        etag: Option<String>,
    },
    /// A body that failed [`parse_catalog`]. The caller keeps its active snapshot.
    Rejected(PricingError),
}

/// Validates a downloaded body the way [`LitellmRefresh`] does: it must parse and pass
/// [`PricingCatalog::check_refresh`] against `active`, the active snapshot's body. Returns the
/// catalog and the body to store, with the models `active` prices and `body` lacks carried
/// forward ([`carry_forward`]).
///
/// # Errors
/// [`PricingError`] when the body is not a usable `LiteLLM` catalog.
pub fn parse_catalog(
    body: Vec<u8>,
    active: Option<&[u8]>,
) -> Result<(PricingCatalog, Vec<u8>), PricingError> {
    let catalog = PricingCatalog::parse(&body)?;
    // An active body that no longer parses cannot be compared with nor carried forward.
    let previous = active.and_then(|active| Some((active, PricingCatalog::parse(active).ok()?)));
    catalog.check_refresh(previous.as_ref().map(|(_, catalog)| catalog))?;
    let Some((active, _)) = previous else {
        return Ok((catalog, body));
    };
    let merged = carry_forward(&body, active)?;
    if merged == body {
        return Ok((catalog, body));
    }
    Ok((PricingCatalog::parse(&merged)?, merged))
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

    /// Conditional GET of the price list, sending `etag` (of the active snapshot) as
    /// `If-None-Match`, and validated against `active`, the active snapshot's body
    /// ([`parse_catalog`]). Parsing is CPU-bound (milliseconds); callers on a latency-sensitive
    /// runtime may want `spawn_blocking`.
    ///
    /// # Errors
    /// [`RemoteError`] on transport failure or a status other than 200 and 304. A bad body is
    /// not an error: it is [`RefreshOutcome::Rejected`].
    pub async fn refresh(
        &self,
        etag: Option<&str>,
        active: Option<&[u8]>,
    ) -> Result<RefreshOutcome, RemoteError> {
        let url = join(&self.base, CATALOG_PATH)?;
        let fetched =
            get_conditional(&self.client, url, &Validators::etag(etag), BODY_LIMIT).await?;
        Ok(match fetched {
            Fetched::NotModified => RefreshOutcome::NotModified,
            Fetched::Body { body, validators } => match parse_catalog(body, active) {
                Ok((catalog, body)) => {
                    if catalog.dropped_rates() > 0 {
                        tracing::warn!(
                            dropped = catalog.dropped_rates(),
                            "LiteLLM snapshot has rate values that are not non-negative numbers"
                        );
                    }
                    RefreshOutcome::New {
                        catalog,
                        body,
                        etag: validators.etag,
                    }
                }
                Err(reason) => {
                    tracing::warn!(%reason, "rejecting LiteLLM snapshot");
                    RefreshOutcome::Rejected(reason)
                }
            },
        })
    }
}
