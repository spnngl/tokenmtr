//! What every command needs before it runs: the platform locations and the app context.

use std::sync::Arc;

use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_app::AppContext;
use tokenmtr_config::ProcessEnv;
use tokenmtr_platform::{ControlTransport, Paths, Platform, PlatformKind};

/// The platform and the config file in force.
pub struct Setup {
    platform: PlatformKind,
    config_path: Utf8PathBuf,
}

impl Setup {
    /// Resolves the platform directories; `config` overrides the config file location.
    pub fn detect(config: Option<&Utf8Path>) -> anyhow::Result<Self> {
        let platform = PlatformKind::detect().context("cannot resolve the platform directories")?;
        let config_path = config.map_or_else(
            || platform.paths().config_file().to_owned(),
            Utf8Path::to_owned,
        );
        Ok(Self {
            platform,
            config_path,
        })
    }

    pub fn paths(&self) -> &Paths {
        self.platform.paths()
    }

    pub fn config_path(&self) -> &Utf8Path {
        &self.config_path
    }

    pub fn platform(&self) -> &PlatformKind {
        &self.platform
    }

    pub fn transport(&self) -> ControlTransport {
        self.platform.control_transport()
    }

    /// Loads the config and opens the store read-only.
    pub fn context(&self) -> anyhow::Result<AppContext> {
        AppContext::load(
            self.paths().clone(),
            self.transport(),
            self.config_path.clone(),
            Arc::new(ProcessEnv),
        )
        .with_context(|| format!("cannot load {}", self.config_path))
    }
}

/// The query clock.
pub fn now() -> Timestamp {
    Timestamp::now()
}
