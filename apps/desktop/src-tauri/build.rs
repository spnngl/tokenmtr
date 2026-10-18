//! Tauri build script.
//!
//! Declares the app commands so each window's capability can allow only the ones it needs
//! (`capabilities/*.json`); the list is shared with `commands.rs`, which tests it against the
//! invoke handler.

use tauri_build::{AppManifest, Attributes};

const COMMANDS: &[&str] = include!("src/command_names.in");

fn main() {
    tauri_build::try_build(Attributes::new().app_manifest(AppManifest::new().commands(COMMANDS)))
        .unwrap_or_else(|error| std::process::exit(report(&error)));
}

#[expect(clippy::print_stderr, reason = "a build script reports through stderr")]
fn report(error: &dyn std::fmt::Display) -> i32 {
    eprintln!("tauri build failed: {error}");
    1
}
