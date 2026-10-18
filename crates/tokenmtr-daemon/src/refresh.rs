//! Daily downloads: ECB exchange rates (D27) and the `LiteLLM` price list (D11).

use std::sync::Arc;

use jiff::Timestamp;
use tokenmtr_core::EventCost;
use tokenmtr_core::{ApiCost, Override, ParsedEvent, PricingCatalog, UsageEvent, estimate};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_remote::{EcbFx, FxFetch, LitellmRefresh, RefreshOutcome};
use tokenmtr_store::{FxSource, Revision, StoreRead};

use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

/// Events read per page while re-pricing.
const PAGE: usize = 1000;

/// Where stored snapshots say they came from.
pub(crate) const PRICING_SOURCE: &str = "litellm";

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

    /// Conditional GET of the price list; a valid new list becomes the active snapshot and
    /// prices the events that had no price (D11).
    pub(crate) async fn refresh_pricing(self: &Arc<Self>) -> Result<PricingResult, DaemonError> {
        let http = self.http()?;
        let source = LitellmRefresh::new(http.client.clone(), http.endpoints.github_raw.clone());
        let etag = lock(&self.run).pricing_etag.clone();
        match source.refresh(etag.as_deref()).await? {
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
                    let version = lock(&ctx.store).put_pricing_snapshot(
                        &body,
                        Timestamp::now(),
                        Some(PRICING_SOURCE),
                    )?;
                    lock(&ctx.run).pricing_etag = etag;
                    let repriced = ctx.reprice_unpriced(&catalog, &overrides)?;
                    tracing::info!(%version, repriced, "price list updated");
                    ctx.after_write(true);
                    Ok(PricingResult::Updated { repriced })
                })
                .await?
            }
        }
    }

    /// Gives a price to every event that has none; priced events keep theirs (D11).
    fn reprice_unpriced(
        &self,
        catalog: &PricingCatalog,
        overrides: &[Override],
    ) -> Result<u64, DaemonError> {
        let mut after = Revision::ZERO;
        let mut repriced = 0_u64;
        loop {
            let page = lock(&self.reader).usage_events_after(after, PAGE)?;
            let Some(last) = page.last().map(|event| event.revision) else {
                return Ok(repriced);
            };
            let full = page.len() == PAGE;
            let updates: Vec<UsageEvent> = page
                .into_iter()
                .filter_map(|event| priced(event.row, catalog, overrides))
                .collect();
            if !updates.is_empty() {
                lock(&self.store).upsert_events(&updates)?;
                repriced += u64::try_from(updates.len()).unwrap_or(u64::MAX);
            }
            if !full {
                return Ok(repriced);
            }
            after = last;
        }
    }
}

/// `event` with an estimate, if it had no price and the catalog has a rate for it.
fn priced(
    mut event: UsageEvent,
    catalog: &PricingCatalog,
    overrides: &[Override],
) -> Option<UsageEvent> {
    if !matches!(event.cost.api_cost(), ApiCost::Unpriced) {
        return None;
    }
    let parsed = ParsedEvent {
        agent: event.agent,
        profile: event.profile.clone(),
        dedup_key: event.dedup_key.clone(),
        at: event.at,
        session: event.session.clone(),
        parent_session: event.parent_session.clone(),
        subagent: event.subagent.clone(),
        is_sidechain: event.is_sidechain,
        cwd: event.cwd.clone(),
        git_branch: event.git_branch.clone(),
        source_file: event.source_file,
        approximate: event.approximate,
        provider: event.provider.clone(),
        model: event.model.clone(),
        kind: event.kind,
        tokens: event.tokens,
        reported_cost: None,
        service_tier: event.service_tier.clone(),
    };
    let estimate = estimate(&parsed, catalog, overrides)?;
    event.cost = EventCost::new(None, Some(estimate));
    Some(event)
}
