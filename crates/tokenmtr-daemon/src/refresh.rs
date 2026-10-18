//! Daily downloads: ECB exchange rates and the `LiteLLM` price list.

use std::sync::Arc;

use jiff::Timestamp;
use tokenmtr_core::pricing::SOURCE_LITELLM;
use tokenmtr_platform::ThreadPriority;
use tokenmtr_remote::{EcbFx, FxFetch, LitellmRefresh, RefreshOutcome};
use tokenmtr_store::{FxSource, RepriceScope, Revision, StoreRead};

use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

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
    /// Downloads rates: the whole history the first time, then the daily file.
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
    /// events that had no price. The `ETag` is stored with the snapshot, so it survives
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
                self.blocking(ThreadPriority::Background, move |ctx| {
                    let version = {
                        let mut store = lock(&ctx.store);
                        let version =
                            store.put_pricing_snapshot(&body, Timestamp::now(), Some(SOURCE_LITELLM))?;
                        store.set_pricing_etag(etag.as_deref())?;
                        version
                    };
                    let repriced = ctx.reprice(RepriceScope::Unpriced)?;
                    tracing::info!(%version, carried = catalog.carried(), repriced, "price list updated");
                    ctx.after_write(true);
                    Ok(PricingResult::Updated { repriced })
                })
                .await?
            }
        }
    }

    /// `tokenmtr reprice [--all]` through the control socket: re-estimates at user-initiated
    /// priority, then publishes the change.
    pub(crate) async fn reprice_now(
        self: &Arc<Self>,
        scope: RepriceScope,
    ) -> Result<u64, DaemonError> {
        self.blocking(ThreadPriority::UserInitiated, move |ctx| {
            let repriced = ctx.reprice(scope)?;
            ctx.after_write(true);
            Ok(repriced)
        })
        .await?
    }

    /// Re-estimates the stored events of `scope` with the active snapshot, through the
    /// ingestor's pricer so the overrides and the parsed catalog are the scan's.
    fn reprice(&self, scope: RepriceScope) -> Result<u64, DaemonError> {
        let mut ingestor = lock(&self.ingestor);
        let mut store = lock(&self.store);
        Ok(ingestor.reprice(&mut store, scope)?)
    }
}
