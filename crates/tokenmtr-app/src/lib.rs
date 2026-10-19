//! What the `tokenmtr` CLI and the desktop shell share: the config → query mapping, a read-only
//! store behind [`AppContext`], reports, backups and restore, erase and the
//! Claude retention switch.
//!
//! The crate is synchronous. Talking to a running daemon is the caller's job
//! (`tokenmtr-control`). Every action here that writes the store holds the daemon lock, so no
//! daemon can start meanwhile: the direct backup, scan and reprice refuse while a daemon runs;
//! erase and restore stop it first and start a kept service again afterwards.

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
pub use guard::{STOP_GRACE, daemon_running, lock_daemon, restore_backup, with_daemon_stopped};
pub use report::{RenderedReport, ReportFormat, ReportRange, ReportRequest, generate_report};
pub use retention::{RETENTION_DAYS, set_claude_retention};
pub use settings::{
    add_project_rule, add_subscription, backup_dir, budget_specs, display_zone, ensure_config_file,
    load_config, query_context, recap_config, set_git_defaults, set_menubar_rings, sink_specs,
    store_path,
};
