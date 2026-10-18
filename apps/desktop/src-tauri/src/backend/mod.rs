//! The store-facing half of the shell: plain functions over an [`AppContext`], no window needed.
//!
//! Every command opens a fresh context ([`Backend::context`]): opening is cheap, the config and
//! the store file are always current, and nothing survives a daemon restart, an erase or a
//! restore holding a stale handle.

pub mod actions;
pub mod queries;
pub mod shell;

use std::sync::{Arc, Mutex, PoisonError};

use camino::Utf8PathBuf;
use tokenmtr_app::{AppContext, EraseReport, EraseScope, load_config};
use tokenmtr_config::ProcessEnv;
use tokenmtr_platform::{
    ControlTransport, Paths, Platform, PlatformKind, ServiceManager, ThreadPriority,
};

use crate::error::CommandError;

/// Held while a context loads (loading creates a missing store; two tasks doing that at once,
/// as on the first launch, would both run the schema and one would fail) and while an erase
/// deletes it, so no load recreates the store halfway.
static LOAD: Mutex<()> = Mutex::new(());

/// Where the config, the store and the control socket are.
#[derive(Debug, Clone)]
pub struct Backend {
    paths: Paths,
    transport: ControlTransport,
    config_path: Utf8PathBuf,
}

impl Backend {
    /// The standard locations of this platform.
    ///
    /// # Errors
    ///
    /// When the platform directories cannot be resolved (no `HOME`).
    pub fn detect() -> Result<Self, CommandError> {
        let platform = PlatformKind::detect().map_err(CommandError::internal)?;
        Ok(Self::new(
            platform.paths().clone(),
            platform.control_transport(),
            platform.paths().config_file().to_owned(),
        ))
    }

    /// Explicit locations (tests).
    #[must_use]
    pub const fn new(paths: Paths, transport: ControlTransport, config_path: Utf8PathBuf) -> Self {
        Self {
            paths,
            transport,
            config_path,
        }
    }

    /// The standard directories.
    #[must_use]
    pub const fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The control socket.
    #[must_use]
    pub const fn transport(&self) -> &ControlTransport {
        &self.transport
    }

    /// The config file.
    #[must_use]
    pub fn config_path(&self) -> &camino::Utf8Path {
        &self.config_path
    }

    /// A fresh context over the config and the store.
    ///
    /// # Errors
    ///
    /// An invalid config, or a store that cannot be opened (newer or older schema).
    pub fn context(&self) -> Result<AppContext, CommandError> {
        let _loading = LOAD.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(AppContext::load(
            self.paths.clone(),
            self.transport.clone(),
            self.config_path.clone(),
            Arc::new(ProcessEnv),
        )?)
    }

    /// Erases with `scope` (D56) without opening the store, so a broken config or store can be
    /// erased too: an unloadable config means the default locations.
    ///
    /// # Errors
    ///
    /// As [`tokenmtr_app::erase`].
    pub fn erase(
        &self,
        scope: EraseScope,
        service: &dyn ServiceManager,
    ) -> Result<EraseReport, CommandError> {
        let _erasing = LOAD.lock().unwrap_or_else(PoisonError::into_inner);
        let config = load_config(&self.config_path, &ProcessEnv)
            .inspect_err(|error| {
                tracing::warn!(%error, "cannot load the config; erasing the default locations");
            })
            .ok();
        Ok(tokenmtr_app::erase(
            &self.paths,
            &self.transport,
            &self.config_path,
            config.as_ref(),
            scope,
            service,
        )?)
    }
}

/// Runs `work` on a blocking thread at user-initiated priority (D47) and returns its result.
///
/// # Errors
///
/// What `work` returns, or an internal error when the thread panicked.
pub async fn blocking<T, F>(work: F) -> Result<T, CommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = tokenmtr_platform::set_thread_priority(ThreadPriority::UserInitiated) {
            tracing::debug!(%error, "cannot raise the worker priority");
        }
        work()
    })
    .await
    .map_err(CommandError::from)?
}

#[cfg(test)]
pub(crate) mod fixture {
    //! A seeded store in a temporary home, shared by the backend tests.

    use camino::Utf8PathBuf;
    use jiff::Timestamp;
    use tempfile::TempDir;
    use tokenmtr_core::{
        AgentId, DedupKey, EventCost, EventId, EventKind, ModelId, ProfileId, ProjectKey,
        SessionId, TokenCounts, UsageEvent, UsdMicros,
    };
    use tokenmtr_platform::{ControlTransport, Paths};
    use tokenmtr_store::{Store, StoreRead as _};

    use super::Backend;

    /// A backend over a temporary home holding one Claude session of two events.
    pub struct Seeded {
        pub backend: Backend,
        pub now: Timestamp,
        _dir: TempDir,
    }

    pub fn ts(text: &str) -> Timestamp {
        text.parse().unwrap_or(Timestamp::UNIX_EPOCH)
    }

    pub fn seeded(config: &str) -> Result<Seeded, Box<dyn std::error::Error>> {
        let dir = tempfile::Builder::new()
            .prefix("tm-shell")
            .tempdir_in("/tmp")?;
        let home = Utf8PathBuf::from_path_buf(dir.path().to_owned())
            .map_err(|path| format!("not UTF-8: {}", path.display()))?;
        let lookup = home.to_string();
        let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| lookup.clone()))?;
        std::fs::create_dir_all(paths.config_dir())?;
        std::fs::write(paths.config_file(), config)?;
        std::fs::create_dir_all(paths.data_dir())?;
        let transport = ControlTransport::unix(paths.data_dir().join("control.sock"));
        let mut store = Store::open(&paths.data_dir().join("usage.db"))?;
        let machine = store.machine_id();
        let events: Vec<UsageEvent> = [
            ("a", "2026-05-04T09:00:00Z", 2_000_000_i64),
            ("b", "2026-05-04T09:30:00Z", 3_000_000),
        ]
        .into_iter()
        .map(
            |(key, at, micros)| -> Result<UsageEvent, Box<dyn std::error::Error>> {
                let dedup_key = DedupKey::try_from(key.to_owned())?;
                Ok(UsageEvent {
                    id: EventId::new(&machine, AgentId::CLAUDE, &dedup_key),
                    agent: AgentId::CLAUDE,
                    profile: ProfileId::default_profile(),
                    dedup_key,
                    at: ts(at),
                    session: SessionId::try_from("s1".to_owned())?,
                    parent_session: None,
                    subagent: None,
                    is_sidechain: false,
                    cwd: Some(Utf8PathBuf::from("/work/acme")),
                    git_branch: None,
                    project: Some(ProjectKey::try_from("acme".to_owned())?),
                    client: None,
                    source_file: None,
                    approximate: false,
                    provider: None,
                    model: ModelId::try_from("claude-sonnet-4".to_owned())?,
                    kind: EventKind::Message,
                    tokens: TokenCounts {
                        output: 500,
                        ..TokenCounts::default()
                    },
                    cost: EventCost::new(Some(UsdMicros::new(micros)), None),
                    service_tier: None,
                })
            },
        )
        .collect::<Result<_, _>>()?;
        store.upsert_events(&events)?;
        drop(store);
        Ok(Seeded {
            backend: Backend::new(paths.clone(), transport, paths.config_file().to_owned()),
            now: ts("2026-05-04T12:00:00Z"),
            _dir: dir,
        })
    }

    pub const CONFIG: &str = "[display]\ntimezone = \"UTC\"\ncurrency = \"USD\"\n";
}
