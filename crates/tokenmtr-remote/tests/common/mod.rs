//! Helpers shared by the integration tests.
#![allow(dead_code, reason = "each test binary uses a subset")]

use std::error::Error;
use std::path::PathBuf;

use reqwest::Url;
use tokenmtr_remote::Endpoints;
use wiremock::MockServer;

pub type TestResult = Result<(), Box<dyn Error>>;

/// The fake OAuth token every test uses; it must never show up in output.
pub const FAKE_TOKEN: &str = "sk-ant-oat01-FAKE-TOKEN-0000000000000000";

pub fn fixture(name: &str) -> Result<Vec<u8>, std::io::Error> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "fixtures",
        "http",
        name,
    ]
    .iter()
    .collect();
    std::fs::read(path)
}

pub fn fixture_text(name: &str) -> Result<String, Box<dyn Error>> {
    Ok(String::from_utf8(fixture(name)?)?)
}

/// Endpoints that all point at `server`.
pub fn endpoints(server: &MockServer) -> Result<Endpoints, Box<dyn Error>> {
    Ok(Endpoints::at(&server.uri())?)
}

/// A base URL nothing listens on: bind a port, then close it.
pub fn closed_port_base() -> Result<Url, Box<dyn Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(Url::parse(&format!("http://127.0.0.1:{port}"))?)
}
