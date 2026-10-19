//! Default locations and the folders the OS guards with consent prompts.

use camino::{Utf8Path, Utf8PathBuf};
use tokenmtr_brand::{
    APP_SUPPORT_DIR_NAME, CACHE_DIR_NAME, CONFIG_DIR_NAME, CONFIG_FILE_NAME, ENV_CONFIG,
    LOG_DIR_NAME,
};

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
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "each OS builds only its own layout; tests build both"
    )
)]
enum Layout {
    /// macOS: `~/Library` and the TCC-protected folders.
    MacOs,
    /// Linux: the XDG base directories; no consent-guarded folders.
    Linux,
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
    /// The layout of the OS this build targets.
    #[cfg(target_os = "linux")]
    const CURRENT: Self = Self::Linux;

    /// `xdg` returns an XDG base directory variable, `None` unless it is set to an absolute path.
    fn dirs(self, home: &Utf8Path, xdg: impl Fn(&str) -> Option<Utf8PathBuf>) -> Dirs {
        match self {
            Self::Linux => {
                let base = |var: &str, fallback: &str, name: &str| {
                    xdg(var).unwrap_or_else(|| home.join(fallback)).join(name)
                };
                Dirs {
                    data: base("XDG_DATA_HOME", ".local/share", APP_SUPPORT_DIR_NAME),
                    logs: base("XDG_STATE_HOME", ".local/state", LOG_DIR_NAME),
                    cache: base("XDG_CACHE_HOME", ".cache", CACHE_DIR_NAME),
                }
            }
            Self::MacOs => {
                let library = home.join("Library");
                Dirs {
                    data: library
                        .join("Application Support")
                        .join(APP_SUPPORT_DIR_NAME),
                    logs: library.join("Logs").join(LOG_DIR_NAME),
                    cache: library.join("Caches").join(CACHE_DIR_NAME),
                }
            }
        }
    }

    fn protected(self, home: &Utf8Path) -> Vec<Utf8PathBuf> {
        match self {
            Self::Linux => Vec::new(),
            Self::MacOs => {
                let library = home.join("Library");
                vec![
                    home.join("Documents"),
                    home.join("Desktop"),
                    home.join("Downloads"),
                    // Photos, Apple Music and TV libraries live here, behind their own consents.
                    home.join("Pictures"),
                    home.join("Movies"),
                    home.join("Music"),
                    // iCloud Drive.
                    library.join("Mobile Documents"),
                    library.join("CloudStorage"),
                    // Other apps' data ("access data from other apps", macOS 14).
                    library.join("Containers"),
                    library.join("Group Containers"),
                    // Removable and network volumes.
                    Utf8PathBuf::from("/Volumes"),
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
    config_home: Utf8PathBuf,
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
        Self::from_lookup_with(Layout::CURRENT, get)
    }

    fn from_lookup_with(
        layout: Layout,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, PathsError> {
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
        let xdg = |var: &str| env(var).map(Utf8PathBuf::from).filter(|p| p.is_absolute());
        let config_override = env(ENV_CONFIG)
            .map(|v| absolute(ENV_CONFIG, v))
            .transpose()?;

        let config_home = xdg("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let config_dir = config_home.join(CONFIG_DIR_NAME);
        let dirs = layout.dirs(&home, xdg);
        Ok(Self {
            layout,
            config_home,
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

    /// `$XDG_CONFIG_HOME`, else `~/.config`: where per-user config of other programs lives
    /// (e.g. systemd user units).
    #[must_use]
    pub fn config_home(&self) -> &Utf8Path {
        &self.config_home
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

    /// Data directory: `~/Library/Application Support/dev.tokenmtr.app` on macOS,
    /// `$XDG_DATA_HOME/dev.tokenmtr.app` (else `~/.local/share/…`) on Linux.
    #[must_use]
    pub fn data_dir(&self) -> &Utf8Path {
        &self.data_dir
    }

    /// Logs directory: `~/Library/Logs/tokenmtr` on macOS, `$XDG_STATE_HOME/tokenmtr` (else
    /// `~/.local/state/tokenmtr`) on Linux.
    #[must_use]
    pub fn logs_dir(&self) -> &Utf8Path {
        &self.logs_dir
    }

    /// Cache directory: `~/Library/Caches/dev.tokenmtr.app` on macOS,
    /// `$XDG_CACHE_HOME/dev.tokenmtr.app` (else `~/.cache/…`) on Linux.
    #[must_use]
    pub fn cache_dir(&self) -> &Utf8Path {
        &self.cache_dir
    }

    /// Default exports directory: `<data_dir>/exports`. Outside every protected folder.
    #[must_use]
    pub fn exports_dir(&self) -> Utf8PathBuf {
        self.data_dir.join("exports")
    }

    /// Folders the OS guards with per-app consent prompts: on macOS Documents, Desktop,
    /// Downloads, Pictures, Movies, Music, iCloud Drive, cloud storage, other apps' containers
    /// and `/Volumes`. Empty on Linux.
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
        self.protected().iter().any(|folder| is_under(path, folder))
    }
}

/// Whether `path` is `folder` or lies under it, comparing components lexically and ASCII
/// case-insensitively (default macOS volumes are case-insensitive). Symlinks and `..` are not
/// resolved: the check must run before the filesystem is touched.
#[must_use]
pub fn is_under(path: &Utf8Path, folder: &Utf8Path) -> bool {
    starts_with_ignore_case(path, folder)
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

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn paths(vars: &[(&str, &str)]) -> Result<Paths, PathsError> {
        Paths::from_lookup(lookup(vars))
    }

    fn macos(vars: &[(&str, &str)]) -> Paths {
        Paths::from_lookup_with(Layout::MacOs, lookup(vars)).unwrap()
    }

    fn linux(vars: &[(&str, &str)]) -> Paths {
        Paths::from_lookup_with(Layout::Linux, lookup(vars)).unwrap()
    }

    #[test]
    fn macos_defaults_follow_d21() {
        let p = macos(&[("HOME", "/Users/me")]);
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
    fn linux_defaults_follow_xdg() {
        let p = linux(&[("HOME", "/home/me")]);
        assert_eq!(p.config_home(), "/home/me/.config");
        assert_eq!(p.config_dir(), "/home/me/.config/tokenmtr");
        assert_eq!(p.config_file(), "/home/me/.config/tokenmtr/config.toml");
        assert_eq!(p.data_dir(), "/home/me/.local/share/dev.tokenmtr.app");
        assert_eq!(p.logs_dir(), "/home/me/.local/state/tokenmtr");
        assert_eq!(p.cache_dir(), "/home/me/.cache/dev.tokenmtr.app");
        assert_eq!(
            p.exports_dir(),
            "/home/me/.local/share/dev.tokenmtr.app/exports"
        );
    }

    #[test]
    fn linux_honors_absolute_xdg_variables_only() {
        let p = linux(&[
            ("HOME", "/home/me"),
            ("XDG_CONFIG_HOME", "/c"),
            ("XDG_DATA_HOME", "/d"),
            ("XDG_STATE_HOME", "/s"),
            ("XDG_CACHE_HOME", "/k"),
        ]);
        assert_eq!(p.config_home(), "/c");
        assert_eq!(p.config_file(), "/c/tokenmtr/config.toml");
        assert_eq!(p.data_dir(), "/d/dev.tokenmtr.app");
        assert_eq!(p.logs_dir(), "/s/tokenmtr");
        assert_eq!(p.cache_dir(), "/k/dev.tokenmtr.app");

        let p = linux(&[
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", "rel"),
            ("XDG_STATE_HOME", ""),
            ("XDG_CACHE_HOME", "./x"),
        ]);
        assert_eq!(p.data_dir(), "/home/me/.local/share/dev.tokenmtr.app");
        assert_eq!(p.logs_dir(), "/home/me/.local/state/tokenmtr");
        assert_eq!(p.cache_dir(), "/home/me/.cache/dev.tokenmtr.app");
    }

    #[test]
    fn macos_ignores_xdg_data_variables() {
        let p = macos(&[("HOME", "/Users/me"), ("XDG_DATA_HOME", "/d")]);
        assert_eq!(
            p.data_dir(),
            "/Users/me/Library/Application Support/dev.tokenmtr.app"
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
        let p = paths(&[("HOME", "/Users/me"), (ENV_CONFIG, "/etc/t.toml")]).unwrap();
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
            paths(&[("HOME", "/h"), (ENV_CONFIG, "c.toml")]),
            Err(PathsError::NotAbsolute { .. })
        ));
    }

    #[test]
    fn linux_has_no_protected_folders() {
        let p = linux(&[("HOME", "/home/me")]);
        assert_eq!(p.protected(), Vec::<Utf8PathBuf>::new());
        assert!(!p.is_protected(Utf8Path::new("/home/me/Documents")));
    }

    #[test]
    fn protected_lists_home_folders_and_volumes() {
        let p = macos(&[("HOME", "/Users/me")]);
        let protected = p.protected();
        assert_eq!(protected.len(), 11);
        assert!(
            protected
                .iter()
                .all(|folder| folder.starts_with(p.home()) || folder == "/Volumes")
        );
        assert!(protected.iter().all(|folder| p.is_protected(folder)));
    }

    #[test]
    fn protected_folders_match_by_component_and_ignore_case() {
        let p = macos(&[("HOME", "/Users/me")]);
        for hit in [
            "/Users/me/Documents",
            "/Users/me/documents/a/b.csv",
            "/Users/me/Desktop/x",
            "/Users/me/Downloads",
            "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/x",
            "/Users/me/Library/CloudStorage/Dropbox/tokenmtr",
            "/Users/me/Pictures/x",
            "/Users/me/Movies",
            "/Users/me/Music/y",
            "/Users/me/Library/Containers/com.apple.mail",
            "/Users/me/Library/Group Containers/g",
            "/Volumes/External/repo",
            "/volumes/x",
        ] {
            assert!(p.is_protected(Utf8Path::new(hit)), "{hit}");
        }
        for miss in [
            "/Users/me/DocumentsOld",
            "/Users/me",
            "/Users/me/Library/Application Support/dev.tokenmtr.app/exports",
            "/Users/other/Documents",
            "/Users/me/src/Pictures",
            "/Vol",
        ] {
            assert!(!p.is_protected(Utf8Path::new(miss)), "{miss}");
        }
        assert!(!p.is_protected(&p.exports_dir()));
    }
}
