//! Thin Tauri shell: maps commands to `tokenmtr-query` and `tokenmtr-control` (D2).

/// Starts the Tauri application.
///
/// # Panics
///
/// Panics when Tauri cannot start; there is nothing to recover to at that point.
#[expect(
    clippy::panic,
    reason = "startup failure is unrecoverable for a GUI process"
)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("error while running TokenMtr: {error}"));
}
