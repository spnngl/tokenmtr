//! Scanning agent files into the store.

use std::sync::Arc;

use jiff::Timestamp;
use tokenmtr_agents::ingest::{ScanPriority, ScanReport, ScanRequest};
use tokenmtr_platform::ThreadPriority;

use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

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
            ctx.after_write(true);
            Ok(report)
        })
        .await?
    }
}
