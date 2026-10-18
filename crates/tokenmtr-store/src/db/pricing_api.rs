//! [`Store`] methods of the pricing snapshots and repricing, in a child module so they reach
//! the store's connection and transaction helper.

use std::collections::BTreeSet;

use tokenmtr_core::PricingVersion;

use super::Store;
use crate::error::StoreError;
use crate::pricing::{self, RepriceCursor, RepriceEvent, RepricePage, RepriceScope, Repriced};

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

    /// Re-estimates up to `limit` events of `scope` after `cursor`, in one write transaction.
    /// `price` returns the new estimate, or `None` to leave the event alone; an estimate equal to
    /// the stored one changes nothing. Only the cost columns change, and only on rows no other
    /// write touched meanwhile; each changed event gets a new revision.
    ///
    /// # Errors
    ///
    /// `SQLite` failures, or a stored row that does not decode.
    pub fn reprice(
        &mut self,
        scope: RepriceScope,
        cursor: RepriceCursor,
        limit: usize,
        mut price: impl FnMut(&RepriceEvent) -> Option<Repriced>,
    ) -> Result<RepricePage, StoreError> {
        self.in_transaction(|conn, now| {
            pricing::reprice_page(conn, now, scope, cursor, limit, &mut price)
        })
    }
}
