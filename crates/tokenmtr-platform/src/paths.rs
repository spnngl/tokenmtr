//! Default locations (D21) and macOS protected folders (D50).

use camino::{Utf8Path, Utf8PathBuf};

/// Directory name under the config and logs roots.
const CONFIG_DIR_NAME: &str = "tokenmtr";
/// Config file name inside the config directory.
const CONFIG_FILE_NAME: &str = "config.toml";
/// Bundle identifier, used as the directory name under Application Support and Caches.
const BUNDLE_ID: &str = "dev.tokenmtr.app";
/// Environment variable that overrides the config file location.
const CONFIG_ENV: &str = "TOKENMTR_CONFIG";

/// Failure to derive [`Paths`] from the environment.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathsError {
    /// `$HOME` is missing or empty.
    #[error("$HOME is not set")]
    HomeUnset,
    /// A variable that must hold an absolute path holds something else.
    #[error("${var} must be an absolute path, got {value:?}")]
    NotAbsolute {
        /// Variable name.
        var: &'static str,
        /// Offending value.
        value: String,
    },
}

/// Where an operating system keeps application files and which folders it guards. One variant
/// per platform of [`PlatformKind`](crate::PlatformKind): everything OS-specific about
/// [`Paths`] lives in these arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// macOS: `~/Library` and the TCC-protected folders (D50).
    #[cfg(target_os = "macos")]
    MacOs,
}

/// The directories a [`Layout`] derives from the home directory.
struct Dirs {
    data: Utf8PathBuf,
    logs: Utf8PathBuf,
    cache: Utf8PathBuf,
}

impl Layout {
    /// The layout of the OS this build targets.
    #[cfg(target_os = "macos")]
    const CURRENT: Self = Self::MacOs;

    fn dirs(self, home: &Utf8Path) -> Dirs {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs => {
                let library = home.join("Library");
                Dirs {
                    data: library.join("Application Support").join(BUNDLE_ID),
                    logs: library.join("Logs").join(CONFIG_DIR_NAME),
                    cache: library.join("Caches").join(BUNDLE_ID),
                }
            }
        }
    }

    fn protected(self, home: &Utf8Path) -> Vec<Utf8PathBuf> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs => {
                let library = home.join("Library");
                vec![
                    home.join("Documents"),
                    home.join("Desktop"),
                    home.join("Downloads"),
                    library.join("Mobile Documents"),
                    library.join("CloudStorage"),
                ]
            }
        }
    }
}

/// Resolved default locations. Construct once at startup and pass by reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    layout: Layout,
    home: Utf8PathBuf,
    config_dir: Utf8PathBuf,
    config_file: Utf8PathBuf,
    data_dir: Utf8PathBuf,
    logs_dir: Utf8PathBuf,
    cache_dir: Utf8PathBuf,
}

impl Paths {
    /// Reads `HOME`, `XDG_CONFIG_HOME` and `TOKENMTR_CONFIG` from the process environment.
    ///
    /// # Errors
    /// See [`Paths::from_lookup`].
    pub fn from_env() -> Result<Self, PathsError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Like [`Paths::from_env`], with an injectable variable source.
    ///
    /// `XDG_CONFIG_HOME` is ignored unless absolute, as the XDG specification requires. Empty
    /// values count as unset.
    ///
    /// # Errors
    /// [`PathsError::HomeUnset`] without `HOME`; [`PathsError::NotAbsolute`] if `HOME` or
    /// `TOKENMTR_CONFIG` is relative (resolve user-typed relative paths before this point).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, PathsError> {
        let env = |key: &str| get(key).filter(|v| !v.is_empty());
        let absolute = |var: &'static str, value: String| {
            let path = Utf8PathBuf::from(&value);
            if path.is_absolute() {
                Ok(path)
            } else {
                Err(PathsError::NotAbsolute { var, value })
            }
        };

        let home = absolute("HOME", env("HOME").ok_or(PathsError::HomeUnset)?)?;
        let xdg_config = env("XDG_CONFIG_HOME")
            .map(Utf8PathBuf::from)
            .filter(|p| p.is_absolute());
        let config_override = env(CONFIG_ENV)
            .map(|v| absolute(CONFIG_ENV, v))
            .transpose()?;

        let config_dir = xdg_config
            .unwrap_or_else(|| home.join(".config"))
            .join(CONFIG_DIR_NAME);
        let layout = Layout::CURRENT;
        let dirs = layout.dirs(&home);
        Ok(Self {
            layout,
            config_file: config_override.unwrap_or_else(|| config_dir.join(CONFIG_FILE_NAME)),
            config_dir,
            data_dir: dirs.data,
            logs_dir: dirs.logs,
            cache_dir: dirs.cache,
            home,
        })
    }

    /// The user's home directory.
    #[must_use]
    pub fn home(&self) -> &Utf8Path {
        &self.home
    }

    /// Config directory: `$XDG_CONFIG_HOME/tokenmtr`, else `~/.config/tokenmtr`.
    #[must_use]
    pub fn config_dir(&self) -> &Utf8Path {
        &self.config_dir
    }

    /// Config file: `$TOKENMTR_CONFIG`, else `<config_dir>/config.toml`.
    #[must_use]
    pub fn config_file(&self) -> &Utf8Path {
        &self.config_file
    }

    /// Data directory: `~/Library/Application Support/dev.tokenmtr.app`.
    #[must_use]
    pub fn data_dir(&self) -> &Utf8Path {
        &self.data_dir
    }

    /// Logs directory: `~/Library/Logs/tokenmtr`.
    #[must_use]
    pub fn logs_dir(&self) -> &Utf8Path {
        &self.logs_dir
    }

    /// Cache directory: `~/Library/Caches/dev.tokenmtr.app`.
    #[must_use]
    pub fn cache_dir(&self) -> &Utf8Path {
        &self.cache_dir
    }

    /// Default exports directory: `<data_dir>/exports`. Outside every protected folder (D50).
    #[must_use]
    pub fn exports_dir(&self) -> Utf8PathBuf {
        self.data_dir.join("exports")
    }

    /// Folders the OS guards with per-app consent prompts (D50): on macOS Documents, Desktop,
    /// Downloads, iCloud Drive and cloud storage.
    #[must_use]
    pub fn protected(&self) -> Vec<Utf8PathBuf> {
        self.layout.protected(&self.home)
    }

    /// Whether `path` is one of the [`Paths::protected`] folders or lies under one.
    ///
    /// The comparison is lexical and ASCII case-insensitive (default macOS volumes are
    /// case-insensitive); symlinks are not resolved, so pass an already-canonical path.
    #[must_use]
    pub fn is_protected(&self, path: &Utf8Path) -> bool {
        self.protected()
            .iter()
            .any(|folder| starts_with_ignore_case(path, folder))
    }
}

fn starts_with_ignore_case(path: &Utf8Path, prefix: &Utf8Path) -> bool {
    let mut components = path.components();
    prefix.components().all(|expected| {
        components
            .next()
            .is_some_and(|c| c.as_str().eq_ignore_ascii_case(expected.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(vars: &[(&str, &str)]) -> Result<Paths, PathsError> {
        Paths::from_lookup(|key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        })
    }

    #[test]
    fn defaults_follow_d21() {
        let p = paths(&[("HOME", "/Users/me")]).unwrap();
        assert_eq!(p.config_dir(), "/Users/me/.config/tokenmtr");
        assert_eq!(p.config_file(), "/Users/me/.config/tokenmtr/config.toml");
        assert_eq!(
            p.data_dir(),
            "/Users/me/Library/Application Support/dev.tokenmtr.app"
        );
        assert_eq!(p.logs_dir(), "/Users/me/Library/Logs/tokenmtr");
        assert_eq!(p.cache_dir(), "/Users/me/Library/Caches/dev.tokenmtr.app");
        assert_eq!(
            p.exports_dir(),
            "/Users/me/Library/Application Support/dev.tokenmtr.app/exports"
        );
    }

    #[test]
    fn xdg_config_home_is_honored_when_absolute() {
        let p = paths(&[("HOME", "/Users/me"), ("XDG_CONFIG_HOME", "/x")]).unwrap();
        assert_eq!(p.config_file(), "/x/tokenmtr/config.toml");
        let p = paths(&[("HOME", "/Users/me"), ("XDG_CONFIG_HOME", "rel")]).unwrap();
        assert_eq!(p.config_dir(), "/Users/me/.config/tokenmtr");
    }

    #[test]
    fn env_override_replaces_only_the_config_file() {
        let p = paths(&[("HOME", "/Users/me"), (CONFIG_ENV, "/etc/t.toml")]).unwrap();
        assert_eq!(p.config_file(), "/etc/t.toml");
        assert_eq!(p.config_dir(), "/Users/me/.config/tokenmtr");
    }

    #[test]
    fn bad_environment_is_rejected() {
        assert_eq!(paths(&[]), Err(PathsError::HomeUnset));
        assert_eq!(paths(&[("HOME", "")]), Err(PathsError::HomeUnset));
        assert!(matches!(
            paths(&[("HOME", "rel")]),
            Err(PathsError::NotAbsolute { var: "HOME", .. })
        ));
        assert!(matches!(
            paths(&[("HOME", "/h"), (CONFIG_ENV, "c.toml")]),
            Err(PathsError::NotAbsolute { .. })
        ));
    }

    #[test]
    fn protected_lists_every_folder_under_home() {
        let p = paths(&[("HOME", "/Users/me")]).unwrap();
        let protected = p.protected();
        assert_eq!(protected.len(), 5);
        assert!(protected.iter().all(|folder| folder.starts_with(p.home())));
        assert!(protected.iter().all(|folder| p.is_protected(folder)));
    }

    #[test]
    fn protected_folders_match_by_component_and_ignore_case() {
        let p = paths(&[("HOME", "/Users/me")]).unwrap();
        for hit in [
            "/Users/me/Documents",
            "/Users/me/documents/a/b.csv",
            "/Users/me/Desktop/x",
            "/Users/me/Downloads",
            "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/x",
            "/Users/me/Library/CloudStorage/Dropbox/tokenmtr",
        ] {
            assert!(p.is_protected(Utf8Path::new(hit)), "{hit}");
        }
        for miss in [
            "/Users/me/DocumentsOld",
            "/Users/me",
            "/Users/me/Library/Application Support/dev.tokenmtr.app/exports",
            "/Users/other/Documents",
        ] {
            assert!(!p.is_protected(Utf8Path::new(miss)), "{miss}");
        }
        assert!(!p.is_protected(&p.exports_dir()));
    }
}
