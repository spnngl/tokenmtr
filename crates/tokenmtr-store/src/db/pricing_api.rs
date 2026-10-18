//! [`Store`] methods of the pricing snapshots and repricing (D11), in a child module so they reach
//! the store's connection and transaction helper.

use std::collections::BTreeSet;

use tokenmtr_core::PricingVersion;

use super::Store;
use crate::error::StoreError;
use crate::pricing::{self, RepriceCursor, RepricePage, Repriced, UnpricedEvent};

impl Store {
    /// Records `etag` as the HTTP `ETag` of the active snapshot, so a restarted daemon still asks
    /// for a conditional download. Storing another snapshot drops it; `None` forgets it.
    ///
    /// # Errors
    ///
    /// `SQLite` failures.
    pub fn set_pricing_etag(&mut self, etag: Option<&str>) -> Result<(), StoreError> {
        pricing::set_etag(&self.conn, etag)
    }

    /// Versions of every stored pricing snapshot.
    ///
    /// # Errors
    ///
    /// `SQLite` failures, or a stored version that is not 12 hex characters.
    pub fn pricing_versions(&self) -> Result<BTreeSet<PricingVersion>, StoreError> {
        pricing::versions(&self.conn)
    }

    /// Estimates up to `limit` events after `cursor` that have neither a reported nor an
    /// estimated cost, in one write transaction. Only the cost columns change, and only on rows
    /// no other write touched meanwhile; each priced event gets a new revision (D17).
    ///
    /// # Errors
    ///
    /// `SQLite` failures, or a stored row that does not decode.
    pub fn reprice_unpriced(
        &mut self,
        cursor: RepriceCursor,
        limit: usize,
        mut price: impl FnMut(&UnpricedEvent) -> Option<Repriced>,
    ) -> Result<RepricePage, StoreError> {
        self.in_transaction(|conn, now| pricing::reprice_page(conn, now, cursor, limit, &mut price))
    }
}
