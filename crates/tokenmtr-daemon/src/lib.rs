//! The `TokenMtr` daemon (D1): the only process that writes the store.
//!
//! [`Daemon::start`] runs everything inside the caller's tokio runtime:
//!
//! - a **scheduler** ([`scheduler`](crate::scheduler)): one task sleeping until the next deadline
//!   of any periodic job, so an idle daemon wakes well under once a second (D47);
//! - a **file watcher** feeding debounced scans of the agent roots (D16) and reloads of the
//!   config file (D22);
//! - one event-driven **export job per sink** with backoff (D47);
//! - the **control server** of `tokenmtr-control` with every handler and the `Subscribe` push;
//! - optional jobs, each running only when its config flag is on: Claude quota polling (D26),
//!   ECB rates (D27), the `LiteLLM` price list (D11), daily backups (D37).
//!
//! All store writes go through one `Mutex<Store>` and run on blocking threads at background
//! priority (user-initiated for control requests), never on the async workers.
//!
//! Tokens only ever live in `secrecy::SecretString`; nothing here formats one (D55).

mod alerts;
mod backup;
mod changes;
mod ctx;
mod daemon;
mod error;
mod export;
mod handler;
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
