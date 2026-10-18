//! Pricing of ingested events.
//!
//! - A cost the agent reported wins over any estimate, with one exception: pi writes
//!   `cost.total = 0` for models it cannot price. A zero reported cost on an event with tokens is
//!   dropped, so the catalog estimate stands in for it, and the event is flagged approximate.
//!   Models that are free (`:free` ids, GitHub Copilot) keep their zero.
//! - An event that is stored already keeps its stored estimate and pricing version while its
//!   inputs (tokens, model, provider, service tier) are unchanged and that version is a stored
//!   snapshot. A new snapshot therefore prices only events that were unpriced. An estimate made by
//!   an override carries the override's version, so editing or removing the override
//!   re-estimates the events it priced.
//! - The store's active snapshot is used, except that the embedded one replaces it when it is
//!   newer (an upgrade) or when the stored body no longer parses. Models the replaced snapshot
//!   priced stay priced ([`carry_forward`]).

use std::collections::BTreeSet;

use jiff::Timestamp;
use tokenmtr_core::pricing::{
    EMBEDDED_LITELLM, SOURCE_EMBEDDED, SOURCE_LITELLM, carry_forward, embedded_fetched_at,
};
use tokenmtr_core::{
    Estimate, Override, ParsedEvent, PriceQuery, PriceSource, PricingCatalog, PricingVersion,
    UsageEvent, estimate,
};
use tokenmtr_store::{
    RepriceCursor, RepriceEvent, RepriceScope, Repriced, Store, StoreRead, StoredPricingSnapshot,
};

use super::error::IngestError;

/// Events re-estimated per write transaction.
const REPRICE_PAGE: usize = 1000;

/// The active snapshot, parsed once per version.
struct Active {
    version: PricingVersion,
    catalog: PricingCatalog,
    /// Versions of every stored snapshot: a stored estimate under one of them is kept.
    snapshots: BTreeSet<PricingVersion>,
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

    /// Follows the store's active snapshot. A store without one, with an unreadable one, or
    /// with one older than the snapshot embedded in the binary gets the embedded one, so
    /// pricing works offline from the first scan.
    ///
    /// Each call reads only the active version; the body is loaded and parsed when it changed.
    pub fn refresh(&mut self, store: &mut Store, now: Timestamp) -> Result<(), IngestError> {
        if let Some(version) = store.active_pricing_version()?
            && self.active.as_ref().map(|a| &a.version) == Some(&version)
        {
            return Ok(());
        }
        let stored = store.active_pricing_snapshot()?;
        let (version, catalog) = match stored {
            Some(snapshot) if !embedded_is_newer(&snapshot) => {
                match PricingCatalog::parse(&snapshot.body) {
                    Ok(catalog) => (snapshot.version, catalog),
                    Err(reason) => {
                        tracing::warn!(%reason, version = %snapshot.version,
                            "stored pricing snapshot does not parse; using the embedded one");
                        store_embedded(store, None, now)?
                    }
                }
            }
            Some(snapshot) => store_embedded(store, Some(&snapshot.body), now)?,
            None => store_embedded(store, None, now)?,
        };
        if catalog.dropped_rates() > 0 {
            tracing::warn!(dropped = catalog.dropped_rates(), %version,
                "pricing snapshot has rate values that are not non-negative numbers");
        }
        let snapshots = store.pricing_versions()?;
        self.active = Some(Active {
            version,
            catalog,
            snapshots,
        });
        Ok(())
    }

    /// Re-estimates the stored events of `scope` with the active snapshot and the overrides
    ///, one write transaction per [`REPRICE_PAGE`] events; returns how many changed.
    ///
    /// An event the active snapshot cannot price keeps its estimate, and under
    /// [`RepriceScope::All`] so does one already estimated from the active snapshot: the store
    /// lacks the per-line surcharge inputs (fast mode, region, web searches), so re-estimating it
    /// could only lose them.
    ///
    /// Ends with [`IngestError::Stopped`] before the next page once `stopped` returns true.
    pub fn reprice(
        &mut self,
        store: &mut Store,
        scope: RepriceScope,
        now: Timestamp,
        stopped: impl Fn() -> bool,
    ) -> Result<u64, IngestError> {
        self.refresh(store, now)?;
        let Some(active) = &self.active else {
            return Ok(0);
        };
        let price = |event: &RepriceEvent| {
            if event
                .estimate
                .as_ref()
                .is_some_and(|stored| stored.pricing_version == active.version)
            {
                return None;
            }
            let query = PriceQuery {
                model: &event.model,
                provider: event.provider.as_ref(),
                tokens: &event.tokens,
                service_tier: event.service_tier.as_deref(),
                modifiers: None,
            };
            estimate(&query, &active.catalog, &self.overrides).map(|priced| Repriced {
                estimate: priced.estimate,
                approximate: priced.approximate,
            })
        };
        let mut cursor = RepriceCursor::default();
        let mut repriced = 0_u64;
        loop {
            if stopped() {
                return Err(IngestError::Stopped);
            }
            let page = store.reprice(scope, cursor, REPRICE_PAGE, price)?;
            repriced = repriced.saturating_add(page.repriced);
            match page.next {
                Some(next) => cursor = next,
                None => return Ok(repriced),
            }
        }
    }

    /// Applies the zero-cost rule to `event` and returns its estimate: the stored one when the
    /// event is already stored with the same inputs, else a fresh one. `None` means unpriced.
    /// Sets `event.approximate` when the estimate guessed a rate.
    pub fn price(
        &self,
        store: &impl StoreRead,
        event: &mut ParsedEvent,
    ) -> Result<Option<Estimate>, IngestError> {
        let zero_reported = event.reported_cost.is_some_and(|cost| cost.get() == 0)
            && event.tokens.total_tokens() > 0
            && !is_free(event);
        if zero_reported {
            event.reported_cost = None;
            event.approximate = true;
        }
        let Some(active) = &self.active else {
            return Ok(None);
        };
        let fresh = estimate(&PriceQuery::of(event), &active.catalog, &self.overrides);
        let by_override = fresh
            .as_ref()
            .is_some_and(|p| p.source == PriceSource::Override);
        let kept = if by_override {
            None
        } else {
            store
                .usage_event_by_key(event.agent, &event.dedup_key)?
                .map(|stored| stored.row)
                .filter(|stored| same_inputs(stored, event))
                .and_then(|stored| {
                    let kept = stored.cost.estimated().cloned()?;
                    active
                        .snapshots
                        .contains(&kept.pricing_version)
                        .then_some((kept, stored.approximate))
                })
        };
        if let Some((kept, approximate)) = kept {
            event.approximate |= approximate;
            return Ok(Some(kept));
        }
        Ok(fresh.map(|priced| {
            if event.reported_cost.is_none() {
                event.approximate |= priced.approximate;
            }
            priced.estimate
        }))
    }
}

/// Whether the vendored snapshot is newer than `snapshot`, a `LiteLLM` or embedded one. Others
/// (tests, manual imports) are never replaced.
fn embedded_is_newer(snapshot: &StoredPricingSnapshot) -> bool {
    let replaceable = matches!(
        snapshot.source.as_deref(),
        Some(SOURCE_EMBEDDED | SOURCE_LITELLM)
    );
    replaceable && embedded_fetched_at().is_some_and(|embedded| embedded > snapshot.fetched_at)
}

/// Stores the embedded snapshot, with the models of `previous` it lacks carried forward, and
/// makes it active.
fn store_embedded(
    store: &mut Store,
    previous: Option<&[u8]>,
    now: Timestamp,
) -> Result<(PricingVersion, PricingCatalog), IngestError> {
    let body = previous
        .and_then(|previous| carry_forward(EMBEDDED_LITELLM, previous).ok())
        .unwrap_or_else(|| EMBEDDED_LITELLM.to_vec());
    let fetched_at = embedded_fetched_at().unwrap_or(now);
    let version = store.put_pricing_snapshot(&body, fetched_at, Some(SOURCE_EMBEDDED))?;
    Ok((version, PricingCatalog::parse(&body)?))
}

/// Models whose zero reported cost is real.
fn is_free(event: &ParsedEvent) -> bool {
    event.model.as_str().ends_with(":free")
        || event
            .provider
            .as_ref()
            .is_some_and(|p| matches!(p.as_str(), "github-copilot" | "github_copilot"))
}

fn same_inputs(stored: &UsageEvent, event: &ParsedEvent) -> bool {
    stored.tokens == event.tokens
        && stored.model == event.model
        && stored.provider == event.provider
        && stored.service_tier == event.service_tier
}

#[cfg(test)]
mod tests {
    use tokenmtr_core::{AgentId, ModelId, UsdMicros};
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

        // Store the event with a stub estimate under the stored snapshot.
        let machine = temp.store().machine_id();
        let stub = Estimate {
            usd: UsdMicros::new(7),
            pricing_version: fresh.pricing_version,
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

    #[test]
    fn an_estimate_from_a_changed_override_is_redone() {
        let mut temp = TempStore::new().unwrap();
        let mut event = parsed_event("k", "2026-03-01T00:00:00Z", 1_000_000, 0);
        let model = event.model.clone();
        let rate = |input: &str| {
            Override::new(
                model.clone(),
                input.parse().unwrap(),
                "0".parse().unwrap(),
                None,
                None,
                None,
            )
            .unwrap()
        };
        let mut pricer = Pricer::new(vec![rate("1")]);
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        let reader = temp.reader().unwrap();
        let first = pricer.price(&reader, &mut event).unwrap().unwrap();
        assert_eq!(first.usd, UsdMicros::new(1_000_000));
        let machine = temp.store().machine_id();
        let stored = event.clone().into_usage_event(&machine, Some(first));
        temp.store_mut().upsert_events(&[stored]).unwrap();

        let mut pricer = Pricer::new(vec![rate("2")]);
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        let second = pricer.price(&reader, &mut event).unwrap().unwrap();
        assert_eq!(second.usd, UsdMicros::new(2_000_000));

        // Without the override, the catalog prices it again.
        let mut pricer = Pricer::new(Vec::new());
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        let catalog = pricer.price(&reader, &mut event).unwrap().unwrap();
        assert_ne!(catalog.usd, UsdMicros::new(1_000_000));
    }

    #[test]
    fn reprice_estimates_unpriced_events_then_all_stale_estimates() {
        let mut temp = TempStore::new().unwrap();
        let machine = temp.store().machine_id();
        let unpriced = parsed_event("a", "2026-03-01T00:00:00Z", 1000, 500);
        let stale = Estimate {
            usd: UsdMicros::new(7),
            pricing_version: PricingVersion::of_body(b"old"),
        };
        let old = parsed_event("b", "2026-03-01T00:00:00Z", 1000, 500)
            .into_usage_event(&machine, Some(stale.clone()));
        temp.store_mut()
            .upsert_events(&[unpriced.into_usage_event(&machine, None), old])
            .unwrap();
        let mut pricer = Pricer::new(Vec::new());
        let store = temp.store_mut();
        let now = Timestamp::UNIX_EPOCH;
        assert_eq!(
            pricer
                .reprice(store, RepriceScope::Unpriced, now, || false)
                .unwrap(),
            1
        );
        assert_eq!(
            pricer
                .reprice(store, RepriceScope::Unpriced, now, || false)
                .unwrap(),
            0
        );
        assert_eq!(
            pricer
                .reprice(store, RepriceScope::All, now, || false)
                .unwrap(),
            1,
            "only `b`"
        );
        assert_eq!(
            pricer
                .reprice(store, RepriceScope::All, now, || false)
                .unwrap(),
            0
        );
        let active = store.active_pricing_version().unwrap().unwrap();
        let b = store
            .usage_event_by_key(AgentId::CLAUDE, &"b".parse().unwrap())
            .unwrap()
            .unwrap()
            .row;
        let estimate = b.cost.estimated().unwrap();
        assert_eq!(estimate.pricing_version, active);
        assert_ne!(estimate.usd, stale.usd);
    }

    #[test]
    fn refresh_follows_a_new_active_version() {
        let mut temp = TempStore::new().unwrap();
        let mut pricer = Pricer::new(Vec::new());
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        let body = br#"{"tokenmtr-test-model": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 1e-06}}"#;
        let version = temp
            .store_mut()
            .put_pricing_snapshot(body, Timestamp::UNIX_EPOCH, Some("test"))
            .unwrap();
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        assert_eq!(pricer.active.as_ref().map(|a| &a.version), Some(&version));
        assert!(pricer.active.as_ref().unwrap().snapshots.contains(&version));
    }

    #[test]
    fn free_models_keep_a_zero_reported_cost() {
        let pricer = Pricer::new(Vec::new());
        let temp = TempStore::new().unwrap();
        let reader = temp.reader().unwrap();
        let mut event = parsed_event("k", "2026-03-01T00:00:00Z", 1000, 500);
        event.model = ModelId::try_from("qwen/qwen3-coder:free").unwrap();
        event.reported_cost = Some(UsdMicros::new(0));
        assert_eq!(pricer.price(&reader, &mut event).unwrap(), None);
        assert_eq!(event.reported_cost, Some(UsdMicros::new(0)));
        assert!(!event.approximate);
    }

    #[test]
    fn a_newer_embedded_snapshot_replaces_an_older_download() {
        let mut temp = TempStore::new().unwrap();
        let old = br#"{"tokenmtr-test-model": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 1e-06}}"#;
        temp.store_mut()
            .put_pricing_snapshot(old, Timestamp::UNIX_EPOCH, Some(SOURCE_LITELLM))
            .unwrap();
        let mut pricer = Pricer::new(Vec::new());
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        let active = temp.store().active_pricing_snapshot().unwrap().unwrap();
        assert_eq!(active.source.as_deref(), Some(SOURCE_EMBEDDED));
        let catalog = PricingCatalog::parse(&active.body).unwrap();
        assert_eq!(catalog.carried(), 1, "the old model stays priced");
    }

    #[test]
    fn an_unreadable_snapshot_falls_back_to_the_embedded_one() {
        let mut temp = TempStore::new().unwrap();
        temp.store_mut()
            .put_pricing_snapshot(b"not json", Timestamp::UNIX_EPOCH, Some("test"))
            .unwrap();
        let mut pricer = Pricer::new(Vec::new());
        pricer
            .refresh(temp.store_mut(), Timestamp::UNIX_EPOCH)
            .unwrap();
        let active = temp.store().active_pricing_snapshot().unwrap().unwrap();
        assert_eq!(active.version, PricingVersion::of_body(EMBEDDED_LITELLM));
    }
}
