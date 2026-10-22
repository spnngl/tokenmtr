//! The `pricing_snapshot` table: every pricing body ever used, so estimates stay reproducible
//!. The active snapshot is `meta.pricing_version`; `meta.pricing_etag` holds
//! `<version> <etag>` of the last download.

use std::collections::BTreeSet;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, named_params, params};
use tokenmtr_core::{Estimate, ModelId, PricingVersion, ProviderId, TokenCounts};

use crate::codec::{RowExt, to_i64, to_limit, to_us};
use crate::error::{CodecError, StoreError};
use crate::events::decode_estimate;
use crate::types::{Dataset, Revision};
use crate::versioned;

/// zstd level for snapshot bodies: they are written rarely and read rarely.
const ZSTD_LEVEL: i32 = 3;

/// A stored pricing snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPricingSnapshot {
    /// The first 12 hex characters of the body's SHA-256.
    pub version: PricingVersion,
    /// When the body was fetched.
    pub fetched_at: Timestamp,
    /// Where the body came from (`litellm`, or `embedded`).
    pub source: Option<String>,
    /// The decompressed body.
    pub body: Vec<u8>,
    /// The HTTP `ETag` of the download that produced this snapshot, if recorded.
    pub etag: Option<String>,
}

/// Which stored events a repricing pass visits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepriceScope {
    /// Events with neither a reported nor an estimated cost: the daemon's pass after a new
    /// snapshot, and `tokenmtr reprice`.
    Unpriced,
    /// Every event: `tokenmtr reprice --all`. Reported costs never change; only estimates do.
    All,
}

/// What the estimate of a stored event depends on, and the estimate it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepriceEvent {
    /// Model as reported.
    pub model: ModelId,
    /// Serving provider, if known.
    pub provider: Option<ProviderId>,
    /// Token columns.
    pub tokens: TokenCounts,
    /// Service tier.
    pub service_tier: Option<String>,
    /// The stored estimate; always `None` under [`RepriceScope::Unpriced`].
    pub estimate: Option<Estimate>,
}

/// A new estimate for a stored event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repriced {
    /// The estimate.
    pub estimate: Estimate,
    /// Whether the estimate guessed a rate; it sets the event's `approximate` flag, never
    /// clears it.
    pub approximate: bool,
}

/// Where a repricing pass resumes. Start from [`RepriceCursor::default`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepriceCursor(i64);

/// One page of a repricing pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepricePage {
    /// Events whose estimate was written.
    pub repriced: u64,
    /// Where the next page starts; `None` once every event of the scope was seen.
    pub next: Option<RepriceCursor>,
}

/// Stores `body` (a no-op when its version exists) and makes it the active snapshot.
pub(crate) fn put(
    conn: &mut Connection,
    body: &[u8],
    fetched_at: Timestamp,
    source: Option<&str>,
) -> Result<PricingVersion, StoreError> {
    let version = PricingVersion::of_body(body);
    let compressed = zstd::encode_all(body, ZSTD_LEVEL)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT OR IGNORE INTO pricing_snapshot (version, fetched_at_us, source, body_zstd, body_len) \
         VALUES (:version, :fetched_at_us, :source, :body_zstd, :body_len)",
        named_params! {
            ":version": version.as_str(),
            ":fetched_at_us": to_us(fetched_at),
            ":source": source,
            ":body_zstd": compressed,
            ":body_len": body_len(body.len())?,
        },
    )?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('pricing_version', ?1) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [version.as_str()],
    )?;
    tx.commit()?;
    Ok(version)
}

fn body_len(len: usize) -> Result<i64, StoreError> {
    to_i64(u64::try_from(len).unwrap_or(u64::MAX), "body_len")
}

/// The snapshot `meta.pricing_version` points at, if one was ever stored.
pub(crate) fn active(conn: &Connection) -> Result<Option<StoredPricingSnapshot>, StoreError> {
    let etag: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'pricing_etag'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    conn.prepare_cached(
        "SELECT p.version, p.fetched_at_us, p.source, p.body_zstd, p.body_len \
         FROM pricing_snapshot p JOIN meta m ON m.key = 'pricing_version' AND m.value = p.version",
    )?
    .query_and_then([], |row| -> Result<StoredPricingSnapshot, StoreError> {
        let compressed: Vec<u8> = row.get("body_zstd")?;
        let body = zstd::decode_all(compressed.as_slice())?;
        let expected: i64 = row.get("body_len")?;
        if body_len(body.len())? != expected {
            return Err(CodecError::Value {
                column: "body_len",
                value: format!("{expected} != {}", body.len()),
            }
            .into());
        }
        let version: String = row.get("version")?;
        let etag = etag
            .as_deref()
            .and_then(|tagged| tagged.strip_prefix(version.as_str()))
            .and_then(|rest| rest.strip_prefix(' '))
            .map(str::to_owned);
        Ok(StoredPricingSnapshot {
            version: PricingVersion::try_from(version).map_err(CodecError::from)?,
            fetched_at: row.ts("fetched_at_us")?,
            source: row.get("source")?,
            body,
            etag,
        })
    })?
    .next()
    .transpose()
}

/// `meta.pricing_version` when it names a stored snapshot.
pub(crate) fn active_version(conn: &Connection) -> Result<Option<PricingVersion>, StoreError> {
    let version: Option<String> = conn
        .prepare_cached(
            "SELECT p.version FROM pricing_snapshot p \
             JOIN meta m ON m.key = 'pricing_version' AND m.value = p.version",
        )?
        .query_row([], |row| row.get(0))
        .optional()?;
    Ok(version
        .map(PricingVersion::try_from)
        .transpose()
        .map_err(CodecError::from)?)
}

/// Records `etag` as the `ETag` of the active snapshot; `None` forgets it.
pub(crate) fn set_etag(conn: &Connection, etag: Option<&str>) -> Result<(), StoreError> {
    match etag {
        Some(etag) => conn.execute(
            "INSERT INTO meta (key, value) \
             SELECT 'pricing_etag', value || ' ' || ?1 FROM meta WHERE key = 'pricing_version' \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [etag],
        )?,
        None => conn.execute("DELETE FROM meta WHERE key = 'pricing_etag'", [])?,
    };
    Ok(())
}

/// Versions of every stored snapshot.
pub(crate) fn versions(conn: &Connection) -> Result<BTreeSet<PricingVersion>, StoreError> {
    conn.prepare_cached("SELECT version FROM pricing_snapshot")?
        .query_and_then([], |row| -> Result<PricingVersion, StoreError> {
            let version: String = row.get(0)?;
            Ok(PricingVersion::try_from(version).map_err(CodecError::from)?)
        })?
        .collect()
}

/// Selects one page of [`RepriceScope`] events. Extra-usage rows carry a reported cost and are
/// never priced; imported rows keep the estimate of the machine that wrote them.
const fn select_sql(scope: RepriceScope) -> &'static str {
    match scope {
        RepriceScope::Unpriced => {
            "SELECT id, revision, model, provider, input_uncached, cache_read, cache_write_5m, \
             cache_write_1h, output, reasoning, service_tier, cost_estimated_usd_micros, \
             pricing_version FROM usage_event \
             WHERE id > ?1 AND cost_reported_usd_micros IS NULL \
             AND cost_estimated_usd_micros IS NULL AND +origin_machine IS NULL \
             ORDER BY id LIMIT ?2"
        }
        RepriceScope::All => {
            "SELECT id, revision, model, provider, input_uncached, cache_read, cache_write_5m, \
             cache_write_1h, output, reasoning, service_tier, cost_estimated_usd_micros, \
             pricing_version FROM usage_event \
             WHERE id > ?1 AND kind <> 'extra_usage' AND +origin_machine IS NULL \
             ORDER BY id LIMIT ?2"
        }
    }
}

/// Writes one new estimate, only if the row still has the revision (and, under
/// [`RepriceScope::Unpriced`], still has no cost) it had when read.
const fn update_sql(scope: RepriceScope) -> &'static str {
    match scope {
        RepriceScope::Unpriced => {
            "UPDATE usage_event SET cost_estimated_usd_micros = ?1, pricing_version = ?2, \
             approximate = approximate OR ?3, revision = ?4, updated_at_us = ?5 \
             WHERE id = ?6 AND revision = ?7 AND cost_reported_usd_micros IS NULL \
             AND cost_estimated_usd_micros IS NULL"
        }
        RepriceScope::All => {
            "UPDATE usage_event SET cost_estimated_usd_micros = ?1, pricing_version = ?2, \
             approximate = approximate OR ?3, revision = ?4, updated_at_us = ?5 \
             WHERE id = ?6 AND revision = ?7"
        }
    }
}

/// Re-estimates up to `limit` events of `scope` after `cursor` with `price`; `None` from `price`
/// keeps the event as it is, and so does an estimate equal to the stored one. Each estimate is a
/// narrow update of the cost columns, guarded by the row's revision, so a concurrent scan is
/// never overwritten; each changed row gets a new revision.
pub(crate) fn reprice_page(
    conn: &Connection,
    now: Timestamp,
    scope: RepriceScope,
    cursor: RepriceCursor,
    limit: usize,
    price: &mut dyn FnMut(&RepriceEvent) -> Option<Repriced>,
) -> Result<RepricePage, StoreError> {
    let rows = conn
        .prepare_cached(select_sql(scope))?
        .query_and_then(
            params![cursor.0, to_limit(limit)?],
            |row| -> Result<(i64, Revision, RepriceEvent), StoreError> {
                let event = RepriceEvent {
                    model: row.id::<ModelId>("model")?,
                    provider: row.id_opt::<ProviderId>("provider")?,
                    tokens: TokenCounts {
                        input_uncached: row.count("input_uncached")?,
                        cache_read: row.count("cache_read")?,
                        cache_write_5m: row.count("cache_write_5m")?,
                        cache_write_1h: row.count("cache_write_1h")?,
                        output: row.count("output")?,
                        reasoning: row.count("reasoning")?,
                    },
                    service_tier: row.get("service_tier")?,
                    estimate: decode_estimate(row)?,
                };
                Ok((row.get("id")?, row.get("revision")?, event))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let next = (rows.len() >= limit)
        .then(|| rows.last().map(|(id, _, _)| RepriceCursor(*id)))
        .flatten();
    let mut revision = versioned::head(conn, Dataset::UsageEvents)?;
    let mut update = conn.prepare_cached(update_sql(scope))?;
    let mut repriced = 0_u64;
    for (id, current, event) in &rows {
        let Some(Repriced {
            estimate,
            approximate,
        }) = price(event)
        else {
            continue;
        };
        if event.estimate.as_ref() == Some(&estimate) {
            continue;
        }
        let bumped = revision.next();
        let changed = update.execute(params![
            estimate.usd.get(),
            estimate.pricing_version.as_str(),
            approximate,
            bumped,
            to_us(now),
            id,
            current,
        ])?;
        if changed > 0 {
            revision = bumped;
            repriced += 1;
        }
    }
    if repriced > 0 {
        conn.execute(
            "UPDATE dataset_revision SET revision = ?1 WHERE dataset = ?2",
            params![revision, Dataset::UsageEvents.as_str()],
        )?;
    }
    Ok(RepricePage { repriced, next })
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use tokenmtr_core::event::EventKind;
    use tokenmtr_core::{
        AgentId, DedupKey, Estimate, EventCost, EventId, ModelId, PricingVersion, ProfileId,
        SessionId, TokenCounts, UsageEvent, UsdMicros,
    };

    use super::{RepriceCursor, RepriceScope, Repriced};
    use crate::{Dataset, StoreRead, TempStore};

    fn event(store: &impl StoreRead, key: &str, reported: Option<i64>) -> UsageEvent {
        let dedup_key = DedupKey::try_from(key.to_owned()).unwrap();
        UsageEvent {
            id: EventId::new(&store.machine_id(), AgentId::CLAUDE, &dedup_key),
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            dedup_key,
            at: Timestamp::UNIX_EPOCH,
            session: SessionId::try_from("s".to_owned()).unwrap(),
            parent_session: None,
            subagent: None,
            is_sidechain: false,
            cwd: None,
            git_branch: None,
            project: None,
            client: None,
            source_file: None,
            approximate: false,
            provider: None,
            model: ModelId::try_from("m".to_owned()).unwrap(),
            kind: EventKind::Message,
            tokens: TokenCounts {
                output: 10,
                ..TokenCounts::default()
            },
            cost: EventCost::new(reported.map(UsdMicros::new), None),
            service_tier: None,
        }
    }

    #[test]
    fn the_etag_follows_the_active_snapshot() {
        let mut temp = TempStore::new().unwrap();
        let store = temp.store_mut();
        store
            .put_pricing_snapshot(b"{}", Timestamp::UNIX_EPOCH, Some("litellm"))
            .unwrap();
        store.set_pricing_etag(Some("\"a b\"")).unwrap();
        let active = store.active_pricing_snapshot().unwrap().unwrap();
        assert_eq!(active.etag.as_deref(), Some("\"a b\""));
        assert_eq!(
            store.active_pricing_version().unwrap(),
            Some(active.version)
        );
        store
            .put_pricing_snapshot(b"{ }", Timestamp::UNIX_EPOCH, Some("litellm"))
            .unwrap();
        assert_eq!(store.active_pricing_snapshot().unwrap().unwrap().etag, None);
        let versions = store.pricing_versions().unwrap();
        assert!(versions.contains(&PricingVersion::of_body(b"{}")));
        assert_eq!(versions.len(), 2);
    }

    #[test]
    fn repricing_touches_only_unpriced_events_and_bumps_their_revision() {
        let mut temp = TempStore::new().unwrap();
        let store = temp.store_mut();
        let events = [
            event(store, "a", None),
            event(store, "b", Some(5)),
            event(store, "c", None),
        ];
        store.upsert_events(&events).unwrap();
        let before = store.head_revision(Dataset::UsageEvents).unwrap();
        let version = PricingVersion::of_body(b"v");
        let price = |_: &super::RepriceEvent| {
            Some(Repriced {
                estimate: Estimate {
                    usd: UsdMicros::new(7),
                    pricing_version: version.clone(),
                },
                approximate: true,
            })
        };
        let page = store
            .reprice(RepriceScope::Unpriced, RepriceCursor::default(), 1, price)
            .unwrap();
        assert_eq!(page.repriced, 1);
        let next = page.next.unwrap();
        let page = store
            .reprice(RepriceScope::Unpriced, next, 1, price)
            .unwrap();
        assert_eq!(page.repriced, 1);
        let page = store
            .reprice(RepriceScope::Unpriced, page.next.unwrap(), 1, price)
            .unwrap();
        assert_eq!((page.repriced, page.next), (0, None));
        assert_eq!(
            store.head_revision(Dataset::UsageEvents).unwrap(),
            before.next().next()
        );
        let stored = |key: &str| {
            store
                .usage_event_by_key(
                    AgentId::CLAUDE,
                    &DedupKey::try_from(key.to_owned()).unwrap(),
                )
                .unwrap()
                .unwrap()
                .row
        };
        let a = stored("a");
        assert_eq!(a.cost.estimated().map(|e| e.usd.get()), Some(7));
        assert!(a.approximate);
        let b = stored("b");
        assert_eq!(b.cost.estimated(), None);
        assert!(!b.approximate);
    }

    #[test]
    fn repricing_all_rewrites_changed_estimates_only() {
        let mut temp = TempStore::new().unwrap();
        let store = temp.store_mut();
        let estimate = |usd| Estimate {
            usd: UsdMicros::new(usd),
            pricing_version: PricingVersion::of_body(b"old"),
        };
        let mut reported = event(store, "a", Some(5));
        reported.cost = EventCost::new(Some(UsdMicros::new(5)), Some(estimate(3)));
        let mut same = event(store, "b", None);
        same.cost = EventCost::new(None, Some(estimate(9)));
        let kept = event(store, "c", None);
        store.upsert_events(&[reported, same, kept]).unwrap();
        let before = store.head_revision(Dataset::UsageEvents).unwrap();
        // Prices everything at 9 under the old version, except the unpriced "c".
        let price = |event: &super::RepriceEvent| {
            event.estimate.as_ref().map(|_| Repriced {
                estimate: estimate(9),
                approximate: false,
            })
        };
        let page = store
            .reprice(RepriceScope::All, RepriceCursor::default(), 10, price)
            .unwrap();
        assert_eq!((page.repriced, page.next), (1, None), "only `a` changed");
        assert_eq!(
            store.head_revision(Dataset::UsageEvents).unwrap(),
            before.next()
        );
        let a = store
            .usage_event_by_key(
                AgentId::CLAUDE,
                &DedupKey::try_from("a".to_owned()).unwrap(),
            )
            .unwrap()
            .unwrap()
            .row;
        assert_eq!(
            a.cost.reported().map(UsdMicros::get),
            Some(5),
            "reported stays"
        );
        assert_eq!(a.cost.estimated().map(|e| e.usd.get()), Some(9));
    }
}
