//! Desktop entry point. All logic lives in the library crate.

// Prevents an extra console window on Windows in release; harmless elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tokenmtr_desktop_lib::run();
}
