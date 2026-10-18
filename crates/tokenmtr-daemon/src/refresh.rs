//! Daily downloads: ECB exchange rates (D27) and the `LiteLLM` price list (D11).

use std::sync::Arc;

use jiff::Timestamp;
use tokenmtr_core::pricing::SOURCE_LITELLM;
use tokenmtr_core::{Override, PriceQuery, PricingCatalog, estimate};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_remote::{EcbFx, FxFetch, LitellmRefresh, RefreshOutcome};
use tokenmtr_store::{FxSource, RepriceCursor, Repriced, Revision, StoreRead};

use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

/// Events priced per write transaction while re-pricing.
const PAGE: usize = 1000;

/// Where downloaded snapshots say they came from.
pub(crate) const PRICING_SOURCE: &str = SOURCE_LITELLM;

/// What a price list refresh did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PricingResult {
    /// The server says the list did not change.
    Unchanged,
    /// The body failed validation; the previous snapshot stays in force.
    Rejected,
    /// A new snapshot was stored; this many events got a price.
    Updated { repriced: u64 },
}

impl Ctx {
    /// Downloads rates: the whole history the first time, then the daily file (D27).
    pub(crate) async fn refresh_fx(self: &Arc<Self>) -> Result<(), DaemonError> {
        let http = self.http()?;
        let ecb = EcbFx::new(http.client.clone(), http.endpoints.ecb.clone());
        let seeded = self
            .blocking(ThreadPriority::Background, |ctx| {
                lock(&ctx.reader)
                    .fx_rates_after(Revision::ZERO, 1)
                    .map(|rates| !rates.is_empty())
            })
            .await??;
        let rates = if seeded {
            let validators = lock(&self.run).fx_validators.clone();
            match ecb.fetch_daily(&validators).await? {
                FxFetch::NotModified => return Ok(()),
                FxFetch::New { rates, validators } => {
                    lock(&self.run).fx_validators = validators;
                    rates
                }
            }
        } else {
            ecb.fetch_history().await?
        };
        self.blocking(ThreadPriority::Background, move |ctx| {
            let stats = lock(&ctx.store).upsert_fx_rates(FxSource::Ecb, &rates)?;
            tracing::info!(?stats, "FX rates stored");
            ctx.after_write(false);
            Ok(())
        })
        .await?
    }

    /// Conditional GET of the price list, validated against the active snapshot; a valid new
    /// list becomes the active snapshot, keeps the models it dropped priced, and prices the
    /// events that had no price (D11). The `ETag` is stored with the snapshot, so it survives
    /// restarts.
    pub(crate) async fn refresh_pricing(self: &Arc<Self>) -> Result<PricingResult, DaemonError> {
        let http = self.http()?;
        let source = LitellmRefresh::new(http.client.clone(), http.endpoints.github_raw.clone());
        let active = self
            .blocking(ThreadPriority::Background, |ctx| {
                lock(&ctx.reader).active_pricing_snapshot()
            })
            .await??;
        let etag = active
            .as_ref()
            .and_then(|snapshot| snapshot.etag.as_deref());
        let body = active.as_ref().map(|snapshot| snapshot.body.as_slice());
        match source.refresh(etag, body).await? {
            RefreshOutcome::NotModified => Ok(PricingResult::Unchanged),
            RefreshOutcome::Rejected(reason) => {
                tracing::warn!(%reason, "the downloaded price list was rejected");
                Ok(PricingResult::Rejected)
            }
            RefreshOutcome::New {
                catalog,
                body,
                etag,
            } => {
                let overrides = self.active().ingest.overrides.clone();
                self.blocking(ThreadPriority::Background, move |ctx| {
                    let version = {
                        let mut store = lock(&ctx.store);
                        let version =
                            store.put_pricing_snapshot(&body, Timestamp::now(), Some(SOURCE_LITELLM))?;
                        store.set_pricing_etag(etag.as_deref())?;
                        version
                    };
                    let repriced = ctx.reprice_unpriced(&catalog, &overrides)?;
                    tracing::info!(%version, carried = catalog.carried(), repriced, "price list updated");
                    ctx.after_write(true);
                    Ok(PricingResult::Updated { repriced })
                })
                .await?
            }
        }
    }

    /// Gives a price to every event that has none; priced events keep theirs (D11). Each page is
    /// one write that changes only the cost columns of rows no scan rewrote meanwhile.
    fn reprice_unpriced(
        &self,
        catalog: &PricingCatalog,
        overrides: &[Override],
    ) -> Result<u64, DaemonError> {
        let mut cursor = RepriceCursor::default();
        let mut repriced = 0_u64;
        loop {
            let page = lock(&self.store).reprice_unpriced(cursor, PAGE, |event| {
                let query = PriceQuery {
                    model: &event.model,
                    provider: event.provider.as_ref(),
                    tokens: &event.tokens,
                    service_tier: event.service_tier.as_deref(),
                    modifiers: None,
                };
                estimate(&query, catalog, overrides).map(|priced| Repriced {
                    estimate: priced.estimate,
                    approximate: priced.approximate,
                })
            })?;
            repriced = repriced.saturating_add(page.repriced);
            match page.next {
                Some(next) => cursor = next,
                None => return Ok(repriced),
            }
        }
    }
}
