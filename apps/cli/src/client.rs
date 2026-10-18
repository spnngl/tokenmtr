//! Talking to the running daemon over the control socket.

use std::future::Future;

use anyhow::Context;
use tokenmtr_control::{Client, ClientInfo};
use tokenmtr_platform::ControlTransport;
use tokenmtr_store::SCHEMA_VERSION;

/// Connects to the daemon and runs `action` on a single-threaded runtime.
pub fn with_daemon<T, F, Fut>(transport: &ControlTransport, action: F) -> anyhow::Result<T>
where
    F: FnOnce(Client) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(async {
        let client = Client::connect(
            transport,
            ClientInfo {
                name: format!("{}-cli", tokenmtr_core::brand::CLI_NAME),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                schema: Some(SCHEMA_VERSION),
            },
        )
        .await
        .context("cannot talk to the daemon")?;
        action(client).await
    })
}
