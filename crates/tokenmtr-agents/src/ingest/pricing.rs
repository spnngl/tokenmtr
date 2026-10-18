//! Pricing of ingested events (D9–D11).
//!
//! - A cost the agent reported wins over any estimate, with one exception: pi writes
//!   `cost.total = 0` for models it cannot price. A zero reported cost on an event with tokens is
//!   dropped, so the catalog estimate stands in for it and the event is flagged approximate.
//! - An event that is stored already keeps its stored estimate and pricing version while its
//!   inputs (tokens, model, provider, service tier) are unchanged. A new snapshot therefore prices
//!   only events that were unpriced; re-estimating the rest is an explicit `reprice` (D11).

use jiff::Timestamp;
use tokenmtr_core::pricing::EMBEDDED_LITELLM;
use tokenmtr_core::{
    Estimate, Override, ParsedEvent, PricingCatalog, PricingVersion, UsageEvent, estimate,
};
use tokenmtr_store::{Store, StoreRead};

use super::error::IngestError;

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
        store: &impl StoreRead,
        event: &mut ParsedEvent,
    ) -> Result<Option<Estimate>, IngestError> {
        let zero_reported = event.reported_cost.is_some_and(|cost| cost.get() == 0)
            && event.tokens.total_tokens() > 0;
        if zero_reported {
            event.reported_cost = None;
        }
        let kept = store
            .usage_event_by_key(event.agent, &event.dedup_key)?
            .map(|stored| stored.row)
            .filter(|stored| same_inputs(stored, event))
            .and_then(|stored| stored.cost.estimated().cloned());
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

fn same_inputs(stored: &UsageEvent, event: &ParsedEvent) -> bool {
    stored.tokens == event.tokens
        && stored.model == event.model
        && stored.provider == event.provider
        && stored.service_tier == event.service_tier
}

#[cfg(test)]
mod tests {
    use tokenmtr_core::UsdMicros;
    use tokenmtr_store::TempStore;

    use super::*;
    use crate::testkit::parsed_event;

    #[test]
    fn a_stored_estimate_is_kept_only_while_the_inputs_are_unchanged() {
        let mut temp = TempStore::new().unwrap();
        let mut pricer = Pricer::new(Vec::new());
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        let mut event = parsed_event("k", "2026-03-01T00:00:00Z", 1000, 500);
        let reader = temp.reader().unwrap();
        let fresh = pricer.price(&reader, &mut event).unwrap().unwrap();
        assert_ne!(
            fresh.usd,
            UsdMicros::new(7),
            "the fixture must differ from the stub"
        );

        // Store the event with a stub estimate from an older snapshot.
        let machine = temp.store().machine_id();
        let stub = Estimate {
            usd: UsdMicros::new(7),
            pricing_version: PricingVersion::of_body(b"old"),
        };
        let stored = event.clone().into_usage_event(&machine, Some(stub.clone()));
        temp.store_mut().upsert_events(&[stored]).unwrap();

        assert_eq!(pricer.price(&reader, &mut event).unwrap(), Some(stub));
        event.tokens.output += 1;
        let repriced = pricer.price(&reader, &mut event).unwrap().unwrap();
        assert!(
            repriced.usd > UsdMicros::new(7),
            "changed inputs are re-estimated"
        );
    }
}
