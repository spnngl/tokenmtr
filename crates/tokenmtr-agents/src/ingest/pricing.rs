//! Pricing of ingested events (D9–D11).
//!
//! - A cost the agent reported wins over any estimate, with one exception: pi writes
//!   `cost.total = 0` for models it cannot price. A zero reported cost on an event with tokens is
//!   dropped, so the catalog estimate stands in for it and the event is flagged approximate.
//! - An event that is stored already keeps its stored estimate and pricing version while its
//!   inputs (tokens, model, provider, service tier) are unchanged. A new snapshot therefore prices
//!   only events that were unpriced; re-estimating the rest is an explicit `reprice` (D11).

use jiff::Timestamp;
use rusqlite::Connection;
use tokenmtr_core::pricing::EMBEDDED_LITELLM;
use tokenmtr_core::{Estimate, Override, ParsedEvent, PricingCatalog, PricingVersion, estimate};
use tokenmtr_store::{Store, StoreRead as _};

use super::error::IngestError;
use super::stored::{self, StoredPricing};

/// The active snapshot, parsed once per version.
struct Active {
    version: PricingVersion,
    catalog: PricingCatalog,
}

/// Prices events against the store's active pricing snapshot.
pub struct Pricer {
    overrides: Vec<Override>,
    active: Option<Active>,
}

impl Pricer {
    /// A pricer applying `overrides` (`[pricing.overrides]`) on top of the catalog.
    #[must_use]
    pub const fn new(overrides: Vec<Override>) -> Self {
        Self {
            overrides,
            active: None,
        }
    }

    /// Follows the store's active snapshot. A store without one gets the snapshot embedded in
    /// the binary (D11), so pricing works offline from the first scan.
    pub fn refresh(&mut self, store: &mut Store, now: Timestamp) -> Result<(), IngestError> {
        let (version, catalog) = if let Some(snapshot) = store.active_pricing_snapshot()? {
            if self.active.as_ref().map(|a| &a.version) == Some(&snapshot.version) {
                return Ok(());
            }
            (snapshot.version, PricingCatalog::parse(&snapshot.body)?)
        } else {
            let version = store.put_pricing_snapshot(EMBEDDED_LITELLM, now, Some("embedded"))?;
            (version, PricingCatalog::embedded()?)
        };
        self.active = Some(Active { version, catalog });
        Ok(())
    }

    /// Applies the zero-cost rule to `event` and returns its estimate: the stored one when the
    /// event is already stored with the same inputs, else a fresh one. `None` means unpriced.
    pub fn price(
        &self,
        conn: &Connection,
        event: &mut ParsedEvent,
    ) -> Result<Option<Estimate>, IngestError> {
        let zero_reported = event.reported_cost.is_some_and(|cost| cost.get() == 0)
            && event.tokens.total_tokens() > 0;
        if zero_reported {
            event.reported_cost = None;
        }
        let kept = stored::event_pricing(conn, event.agent, &event.dedup_key)?
            .filter(|stored| same_inputs(stored, event))
            .and_then(|stored| stored.estimate);
        let estimate = match kept {
            Some(kept) => Some(kept),
            None => self
                .active
                .as_ref()
                .and_then(|a| estimate(event, &a.catalog, &self.overrides)),
        };
        if zero_reported && estimate.is_some() {
            event.approximate = true;
        }
        Ok(estimate)
    }
}

fn same_inputs(stored: &StoredPricing, event: &ParsedEvent) -> bool {
    stored.tokens == event.tokens
        && stored.model == event.model.as_str()
        && stored.provider.as_deref()
            == event
                .provider
                .as_ref()
                .map(tokenmtr_core::ProviderId::as_str)
        && stored.service_tier == event.service_tier
}
