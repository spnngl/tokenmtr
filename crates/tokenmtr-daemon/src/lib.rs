//! The `TokenMtr` daemon: the store's writer while it runs. Other writers (`tokenmtr-app` erase,
//! restore, direct scans) take the daemon lock, so they run only when no daemon does.
//!
//! [`Daemon::start`] runs everything inside the caller's tokio runtime:
//!
//! - a **scheduler** ([`scheduler`](crate::scheduler)): one task sleeping until the next deadline
//!   of any periodic job, so an idle daemon wakes well under once a second;
//! - a **file watcher** feeding debounced scans of the agent roots and reloads of the
//!   config file;
//! - one event-driven **export job per sink** with backoff;
//! - one **import job per enabled `sqlite` source** (`import`): a pass over the files other
//!   machines exported, on the source'"'"'s interval and when a file under its folders changes;
//! - the **metrics outputs** (`metrics`): one event-driven refresh job folding the store into a
//!   snapshot per output, one scrape server per `[[metrics.exporters]]` entry and one push job
//!   per `[[metrics.remote_write]]` entry, all armed only while configured;
//! - the **control server** of `tokenmtr-control` with every handler and the `Subscribe` push;
//! - optional jobs, each running only when its config flag is on: Claude quota polling,
//!   ECB rates, the `LiteLLM` price list, daily backups.
//!
//! Store writes go through one `Mutex<Store>` (exports and imports have their own connections)
//! and run on blocking threads at background
//! priority (user-initiated for control requests), never on the async workers.
//!
//! Tokens only ever live in `secrecy::SecretString`; nothing here formats one.

mod alerts;
mod backup;
mod changes;
mod ctx;
mod daemon;
mod error;
mod export;
mod handler;
mod import;
mod metrics;
mod quota;
mod refresh;
mod reload;
mod scan;
mod scheduler;
mod settings;
mod timetable;
mod watcher;

pub use daemon::Daemon;
pub use error::DaemonError;
pub use settings::{Settings, Timing};
