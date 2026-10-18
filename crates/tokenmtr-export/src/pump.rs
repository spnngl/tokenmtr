//! The cursor pump (D17): moves rows from the store to sinks, at least once.
//!
//! For each sink and dataset the pump reads `revision > cursor ORDER BY revision LIMIT batch`,
//! redacts (D40), hands the batch to the sink and moves the cursor to the batch's last revision
//! **only after the sink returned `Ok`**, i.e. after the rows are durable. A crash between delivery
//! and the cursor move redelivers the batch; sinks are idempotent on the row id, so nothing is
//! duplicated. Every attempt is recorded as a `sink_run` for health (D41). A failing sink stops its
//! own run and never touches the others.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use jiff::Timestamp;
use tokenmtr_core::SinkId;
use tokenmtr_store::{Dataset, SinkRun, Store, StoreRead};

use crate::error::ExportError;
use crate::project::fetch_batch;
use crate::redact::Redactor;
use crate::sink::{ConfiguredSink, Sink};

/// Rows per batch unless configured otherwise.
pub const DEFAULT_BATCH_SIZE: NonZeroUsize = match NonZeroUsize::new(1_000) {
    Some(size) => size,
    None => NonZeroUsize::MIN,
};

/// Tunables of [`export_pending`].
#[derive(Debug, Clone, Copy)]
pub struct PumpOptions {
    /// Most rows read and delivered per batch.
    pub batch_size: NonZeroUsize,
    /// Time source for `sink_run` timestamps.
    pub clock: fn() -> Timestamp,
}

impl Default for PumpOptions {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            clock: Timestamp::now,
        }
    }
}

/// What one export run did for one sink.
#[derive(Debug)]
pub struct SinkRunOutcome {
    /// The sink.
    pub sink: SinkId,
    /// Rows delivered per dataset during this run.
    pub delivered: BTreeMap<Dataset, u64>,
    /// Why the run stopped early; `None` when every dataset is caught up.
    pub error: Option<ExportError>,
}

impl SinkRunOutcome {
    /// Total rows delivered.
    #[must_use]
    pub fn rows(&self) -> u64 {
        self.delivered.values().sum()
    }
}

/// Exports everything past each sink's cursors. Returns one outcome per sink, in order.
///
/// Sinks are independent: an error in one is reported in its outcome and the next sink still runs.
pub fn export_pending(
    store: &mut Store,
    sinks: &mut [ConfiguredSink],
    options: &PumpOptions,
) -> Vec<SinkRunOutcome> {
    let key = store.redaction_key().clone();
    sinks
        .iter_mut()
        .map(|sink| {
            let mut delivered = BTreeMap::new();
            let error = export_sink(store, &key, sink, options, &mut delivered).err();
            SinkRunOutcome {
                sink: sink.spec.id.clone(),
                delivered,
                error,
            }
        })
        .collect()
}

fn export_sink(
    store: &mut Store,
    key: &tokenmtr_store::RedactionKey,
    sink: &mut ConfiguredSink,
    options: &PumpOptions,
    delivered: &mut BTreeMap<Dataset, u64>,
) -> Result<(), ExportError> {
    let limit = options.batch_size.get();
    let id = sink.spec.id.clone();
    for &dataset in &sink.spec.datasets {
        loop {
            let started = (options.clock)();
            let after = store.sink_cursor(&id, dataset)?;
            let mut batch = match fetch_batch(&*store, dataset, after, limit) {
                Ok(batch) => batch,
                Err(error) => {
                    record_failure(store, &id, dataset, started, options, &error);
                    return Err(error);
                }
            };
            let Some(last) = batch.last_revision() else {
                break;
            };
            let rows = u64::try_from(batch.rows.len()).unwrap_or(u64::MAX);
            Redactor::new(key, &sink.spec.redact).apply(&mut batch);
            match sink.kind.write(&batch) {
                Ok(()) => {
                    store.record_sink_run(&SinkRun {
                        sink: id.clone(),
                        dataset,
                        started,
                        finished: (options.clock)(),
                        rows,
                        error: None,
                    })?;
                    store.set_sink_cursor(&id, dataset, last)?;
                    *delivered.entry(dataset).or_default() += rows;
                }
                Err(error) => {
                    let error = ExportError::from(error);
                    record_failure(store, &id, dataset, started, options, &error);
                    return Err(error);
                }
            }
            if rows < u64::try_from(limit).unwrap_or(u64::MAX) {
                break;
            }
        }
    }
    Ok(())
}

/// Records a failed attempt; a store failure here only costs the history row, never masks `error`.
fn record_failure(
    store: &mut Store,
    sink: &SinkId,
    dataset: Dataset,
    started: Timestamp,
    options: &PumpOptions,
    error: &ExportError,
) {
    let run = SinkRun {
        sink: sink.clone(),
        dataset,
        started,
        finished: (options.clock)(),
        rows: 0,
        error: Some(error.to_string()),
    };
    if let Err(record_error) = store.record_sink_run(&run) {
        tracing::warn!(%sink, %dataset, %record_error, "could not record failed sink run");
    }
}
