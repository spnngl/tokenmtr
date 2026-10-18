//! Behaviour of the daemon that needs its internals.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use camino::Utf8PathBuf;
use secrecy::SecretString;
use tokenmtr_platform::{ControlTransport, Paths, SecretError, SecretStore};
use tokenmtr_store::StoreReader;

use super::*;

struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _service: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
}

/// A scan holds the writer connection (`Ctx::store`) for as long as it runs. The heartbeat must
/// keep beating meanwhile, or the app raises `daemon_stale` during a long cold scan (D36).
#[tokio::test]
async fn the_heartbeat_keeps_beating_while_a_scan_holds_the_writer() {
    // `/tmp` keeps the control socket path under the `sun_path` limit.
    let dir = tempfile::Builder::new()
        .prefix("tm")
        .tempdir_in("/tmp")
        .unwrap();
    let home = Utf8PathBuf::from_path_buf(dir.path().canonicalize().unwrap()).unwrap();
    let lookup_home = home.clone();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| lookup_home.to_string())).unwrap();
    let config_path = home.join("config.toml");
    std::fs::write(
        &config_path,
        "[daemon]\nwatch = false\n[quota]\nenabled = false\n[fx]\nenabled = false\n\
         [pricing]\nauto_update = false\n[backup]\nenabled = false\n",
    )
    .unwrap();
    let store_path = paths.data_dir().join("usage.db");
    let transport = ControlTransport::unix(paths.data_dir().join("control.sock"));
    let mut settings = Settings::new(paths, config_path, transport, Arc::new(NoSecrets));
    settings.timing.heartbeat = Duration::from_millis(50);
    settings.timing.min_slack = Duration::from_millis(10);
    let daemon = Daemon::start(settings).await.unwrap();
    let reader = StoreReader::open(&store_path).unwrap();

    let beat = |reader: &StoreReader| reader.heartbeat().unwrap();
    wait_for(|| beat(&reader).is_some()).await;

    // Hold the writer like a scan does, from another thread so no guard crosses an await.
    let ctx = Arc::clone(&daemon.ctx);
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let scan = std::thread::spawn(move || {
        let _writer = lock(&ctx.store);
        held_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    held_rx.recv().unwrap();

    let before = beat(&reader);
    wait_for(|| beat(&reader) > before).await;

    release_tx.send(()).unwrap();
    scan.join().unwrap();
    daemon.shutdown().await.unwrap();
}

/// Polls `condition` for up to ten seconds.
async fn wait_for(mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not met within 10s");
}
