//! `doctor`: everything that can be wrong, collected without failing on the first problem.

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use tokenmtr_agents::ingest::{DoctorReport, IngestSettings, Ingestor};
use tokenmtr_config::Env;
use tokenmtr_platform::{ControlTransport, Paths, ServiceManager, ServiceStatus};
use tokenmtr_store::{SCHEMA_VERSION, StoreRead, StoreReader};

use crate::context::external_dirs;
use crate::guard::daemon_running;
use crate::settings::{load_config, store_path};

/// What `doctor` found. A problem fills its own `*_error` field; the rest is still checked.
#[derive(Debug, Serialize)]
pub struct Diagnosis {
    /// The config file (it may not exist: the defaults apply).
    pub config_path: Utf8PathBuf,
    /// Why the config is invalid.
    pub config_error: Option<String>,
    /// The store file, once the config is known.
    pub store_path: Option<Utf8PathBuf>,
    /// The store's schema version.
    pub store_schema: Option<u32>,
    /// The schema this build supports.
    pub supported_schema: u32,
    /// Why the store cannot be read (missing, newer schema, corrupt).
    pub store_error: Option<String>,
    /// Whether a daemon answers on the control socket.
    pub daemon_running: bool,
    /// The background service: `not_installed`, `stopped`, `loaded` or `running`.
    pub service: Option<String>,
    /// Folders macOS protects: the daemon needs a grant to read or write inside them.
    pub protected_folders: Vec<Utf8PathBuf>,
    /// Agent files, checkpoints, open issues and retention (needs a valid config and store).
    pub ingest: Option<DoctorReport>,
}

/// Collects the diagnosis. Never fails: every failure is a field.
#[must_use]
pub fn diagnose(
    paths: &Paths,
    transport: &ControlTransport,
    config_path: &Utf8Path,
    env: &dyn Env,
    service: &dyn ServiceManager,
) -> Diagnosis {
    let mut diagnosis = Diagnosis {
        config_path: config_path.to_owned(),
        config_error: None,
        store_path: None,
        store_schema: None,
        supported_schema: SCHEMA_VERSION,
        store_error: None,
        daemon_running: daemon_running(transport),
        service: service.status().ok().map(|status| label(status).to_owned()),
        protected_folders: paths.protected(),
        ingest: None,
    };
    let config = match load_config(config_path, env) {
        Ok(config) => config,
        Err(error) => {
            diagnosis.config_error = Some(error.to_string());
            return diagnosis;
        }
    };
    let store = store_path(paths, &config);
    diagnosis.store_path = Some(store.clone());
    if !store.exists() {
        diagnosis.store_error = Some("the store does not exist yet; the daemon creates it".into());
        return diagnosis;
    }
    let reader = match StoreReader::open(&store) {
        Ok(reader) => reader,
        Err(error) => {
            diagnosis.store_error = Some(error.to_string());
            return diagnosis;
        }
    };
    diagnosis.store_schema = reader.schema_version().ok();
    let ingest = Ingestor::open(IngestSettings::from_config(&config, paths, env), &store)
        .and_then(|ingestor| ingestor.doctor(&reader, &external_dirs(paths, &config)));
    match ingest {
        Ok(report) => diagnosis.ingest = Some(report),
        Err(error) => diagnosis.store_error = Some(error.to_string()),
    }
    diagnosis
}

const fn label(status: ServiceStatus) -> &'static str {
    match status {
        ServiceStatus::NotInstalled => "not_installed",
        ServiceStatus::Stopped => "stopped",
        ServiceStatus::Loaded => "loaded",
        ServiceStatus::Running { .. } => "running",
    }
}
