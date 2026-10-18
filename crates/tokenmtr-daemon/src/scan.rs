//! Scanning agent files into the store.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use tokenmtr_agents::ingest::{ScanPriority, ScanReport, ScanRequest};
use tokenmtr_platform::ThreadPriority;
use tokio::time::Instant;

use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

/// How often scans refresh the store's planner statistics. Scans already wake the daemon, so this
/// adds no wakeup.
const OPTIMIZE_EVERY: Duration = Duration::from_hours(24);

/// Changed events after which a scan refreshes the statistics right away: a first import or a
/// backfill can grow the store tenfold.
const LARGE_SCAN_EVENTS: u64 = 10_000;

impl Ctx {
    /// Runs one scan on a blocking thread at the request's priority, then reacts to what
    /// it wrote.
    pub(crate) async fn scan(
        self: &Arc<Self>,
        request: ScanRequest,
    ) -> Result<ScanReport, DaemonError> {
        let priority = match request.priority {
            ScanPriority::UserInitiated => ThreadPriority::UserInitiated,
            ScanPriority::Background => ThreadPriority::Background,
        };
        self.blocking(priority, move |ctx| {
            let report = {
                let mut ingestor = lock(&ctx.ingestor);
                let mut store = lock(&ctx.store);
                ingestor.scan(&mut store, &request)?
            };
            lock(&ctx.run).last_scan_at = Some(Timestamp::now());
            ctx.optimize_if_due(report.events_changed());
            ctx.after_write(true);
            Ok(report)
        })
        .await?
    }

    /// Refreshes the planner statistics after a large scan or once a day. Cheap when no table
    /// needs it; a failure only costs query speed, so it is logged.
    fn optimize_if_due(&self, events_changed: u64) {
        let now = Instant::now();
        let due = {
            let mut run = lock(&self.run);
            let due = events_changed >= LARGE_SCAN_EVENTS
                || run
                    .optimized_at
                    .is_none_or(|at| now.duration_since(at) >= OPTIMIZE_EVERY);
            if due {
                run.optimized_at = Some(now);
            }
            due
        };
        if due && let Err(error) = lock(&self.store).optimize() {
            tracing::warn!(%error, "cannot refresh the store's statistics");
        }
    }
}
