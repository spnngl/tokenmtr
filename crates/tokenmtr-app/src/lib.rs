//! What the `tokenmtr` CLI and the desktop shell share: the config → query mapping, a read-only
//! store behind [`AppContext`], reports (D46), backups and restore (D37), erase (D56) and the
//! Claude retention switch (D53).
//!
//! The crate is synchronous. Talking to a running daemon is the caller's job
//! (`tokenmtr-control`); every action here that writes the store refuses while one runs (D1).

mod backup;
mod context;
mod diagnose;
mod erase;
mod error;
mod guard;
mod report;
mod retention;
mod settings;

pub use backup::{BackupFile, backup_files, backup_path, list_backups, prune_backups};
pub use context::{
    AccountSummary, AppContext, FxSummary, PricingSnapshotInfo, PricingSummary, QuotaReport,
    UnpricedModel, sink_dirs,
};
pub use diagnose::{Diagnosis, diagnose};
pub use erase::{EraseReport, EraseScope, erase};
pub use error::AppError;
pub use guard::{daemon_running, ensure_daemon_stopped, restore_backup};
pub use report::{RenderedReport, ReportFormat, ReportRange, ReportRequest, generate_report};
pub use retention::{RETENTION_DAYS, set_claude_retention};
pub use settings::{
    add_project_rule, add_subscription, backup_dir, budget_specs, display_zone, load_config,
    query_context, recap_config, sink_specs, store_path,
};
