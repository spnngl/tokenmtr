//! Every product-name constant, in one place (PLAN.md §0 naming).

/// Display name, for window titles and tooltips.
pub const DISPLAY_NAME: &str = "TokenMtr";
/// CLI and daemon binary name.
pub const CLI_NAME: &str = "tokenmtr";
/// Tauri main binary name; it must differ from the CLI (`bundle.externalBin`).
pub const DESKTOP_BINARY_NAME: &str = "tokenmtr-desktop";
/// macOS bundle identifier.
pub const BUNDLE_ID: &str = "dev.tokenmtr.app";
/// launchd label of the daemon `LaunchAgent`.
pub const LAUNCHD_LABEL: &str = "dev.tokenmtr.daemon";
/// Directory name under `$XDG_CONFIG_HOME` / `~/.config`.
pub const CONFIG_DIR_NAME: &str = "tokenmtr";
/// Config file name inside [`CONFIG_DIR_NAME`].
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// Directory name under `~/Library/Application Support` (equals the bundle id).
pub const APP_SUPPORT_DIR_NAME: &str = BUNDLE_ID;
/// Directory name under `~/Library/Logs`.
pub const LOG_DIR_NAME: &str = "tokenmtr";
/// Environment variable overriding the config path.
pub const ENV_CONFIG: &str = "TOKENMTR_CONFIG";
/// Environment variable holding the Postgres sink URL.
pub const ENV_PG_URL: &str = "TOKENMTR_PG_URL";
