//! Reading Claude Code's OAuth credentials, read-only.
//!
//! The daemon never refreshes or writes the token: Claude Code owns it. The access token lives
//! only in a [`SecretString`]; no error here carries credential text.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokenmtr_platform::{SecretError, SecretStore};

use crate::account::{AccountInfo, plan_from_claude};

/// Keychain service Claude Code uses for the default config dir.
pub const DEFAULT_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
/// Credentials file name inside a Claude config dir.
const CREDENTIALS_FILE: &str = ".credentials.json";
/// Hex characters of the config-dir hash in a non-default Keychain service name.
const CONFIG_DIR_HASH_LEN: usize = 8;

/// The Keychain service Claude Code uses when `CLAUDE_CONFIG_DIR` is set to `config_dir`:
/// `Claude Code-credentials-<first 8 hex of sha256(config_dir)>`.
///
/// Derived from Claude Code's behaviour as remembered, **not verified on a real install**:
/// Claude Code ships as compiled bytecode, so the derivation cannot be read from the package, and
/// no install with a custom config dir was at hand. The hash input is the path string exactly as
/// the variable holds it (Claude Code also NFC-normalizes it, which only matters for non-ASCII paths). Override
/// with [`CredentialLocation::with_keychain_service`] when the real name differs.
#[must_use]
pub fn keychain_service_for_config_dir(config_dir: &Utf8Path) -> String {
    let digest = Sha256::digest(config_dir.as_str().as_bytes());
    let hash: String = digest
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .take(CONFIG_DIR_HASH_LEN)
        .map(|nibble| char::from_digit(u32::from(nibble), 16).unwrap_or('0'))
        .collect();
    format!("{DEFAULT_KEYCHAIN_SERVICE}-{hash}")
}

/// One place a credentials JSON document can be read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// A generic-password Keychain item, by service name.
    Keychain {
        /// The service name.
        service: String,
    },
    /// A `.credentials.json` file.
    File {
        /// Its path.
        path: Utf8PathBuf,
    },
}

/// Where one Claude profile's credentials may live, in lookup order (first hit wins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialLocation {
    sources: Vec<CredentialSource>,
}

impl CredentialLocation {
    /// The `default` profile: Keychain `Claude Code-credentials`, then
    /// `<home>/.claude/.credentials.json`.
    #[must_use]
    pub fn default_profile(home: &Utf8Path) -> Self {
        Self {
            sources: vec![
                CredentialSource::Keychain {
                    service: DEFAULT_KEYCHAIN_SERVICE.to_owned(),
                },
                CredentialSource::File {
                    path: home.join(".claude").join(CREDENTIALS_FILE),
                },
            ],
        }
    }

    /// Any other profile: `<config_dir>/.credentials.json`, then the Keychain item Claude
    /// Code names after the config dir ([`keychain_service_for_config_dir`]).
    #[must_use]
    pub fn config_dir(config_dir: &Utf8Path) -> Self {
        Self {
            sources: vec![
                CredentialSource::File {
                    path: config_dir.join(CREDENTIALS_FILE),
                },
                CredentialSource::Keychain {
                    service: keychain_service_for_config_dir(config_dir),
                },
            ],
        }
    }

    /// Replaces the service name of every Keychain source, for a profile whose item name is
    /// configured explicitly.
    #[must_use]
    pub fn with_keychain_service(mut self, service: &str) -> Self {
        for source in &mut self.sources {
            if let CredentialSource::Keychain { service: current } = source {
                service.clone_into(current);
            }
        }
        self
    }

    /// The same location without its Keychain sources, for a profile whose Keychain access was
    /// denied.
    #[must_use]
    pub fn without_keychain(&self) -> Self {
        Self {
            sources: self
                .sources
                .iter()
                .filter(|source| matches!(source, CredentialSource::File { .. }))
                .cloned()
                .collect(),
        }
    }

    /// The sources in lookup order.
    #[must_use]
    pub fn sources(&self) -> &[CredentialSource] {
        &self.sources
    }
}

/// Failure to read credentials. None of the variants carries credential text.
#[derive(Debug, thiserror::Error)]
pub enum CredentialsError {
    /// The secret store refused or failed.
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// The credentials file exists but cannot be read.
    #[error("cannot read credentials file {path}: {source}")]
    Io {
        /// The file.
        path: Utf8PathBuf,
        /// The cause.
        #[source]
        source: std::io::Error,
    },
    /// The credentials file is not a regular file, or other users may write it, so its token
    /// cannot be trusted. Claude Code writes it `0600`.
    #[error("refusing credentials file {path}: {reason}")]
    Insecure {
        /// The file.
        path: Utf8PathBuf,
        /// Why it was refused.
        reason: &'static str,
    },
    /// A credentials document is not the JSON Claude Code writes. The parser's message is
    /// dropped on purpose: it can quote input.
    #[error("credentials are not valid Claude Code credentials JSON")]
    Malformed,
}

impl CredentialsError {
    /// Whether the Keychain refused access ([`SecretError::Denied`]).
    #[must_use]
    pub fn is_keychain_denied(&self) -> bool {
        matches!(self, Self::Secret(SecretError::Denied))
    }
}

/// `{"claudeAiOauth": {...}}`; every other key is ignored.
#[derive(Debug, Deserialize)]
struct RawCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<RawOauth>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOauth {
    access_token: Option<SecretString>,
    /// Milliseconds since the epoch. Read leniently: a value of another type means "unknown".
    expires_at: Option<serde_json::Value>,
    subscription_type: Option<String>,
    rate_limit_tier: Option<String>,
}

/// Whether a Keychain lookup may show the OS consent prompt.
#[derive(Debug, Clone, Copy)]
enum Prompt {
    Never,
    Allowed,
}

/// A Claude OAuth access token plus the non-secret account fields stored beside it.
///
/// `Debug` is safe to log: [`SecretString`] prints `[REDACTED]`.
#[derive(Debug)]
pub struct ClaudeCredentials {
    access_token: SecretString,
    expires_at: Option<Timestamp>,
    account: AccountInfo,
}

impl ClaudeCredentials {
    /// Parses one credentials JSON document. `Ok(None)` when it holds no usable
    /// `claudeAiOauth.accessToken` (signed out).
    ///
    /// # Errors
    /// [`CredentialsError::Malformed`] when the text is not JSON of the expected shape.
    pub fn parse(json: &str) -> Result<Option<Self>, CredentialsError> {
        let raw: RawCredentials =
            serde_json::from_str(json).map_err(|_| CredentialsError::Malformed)?;
        let Some(oauth) = raw.claude_ai_oauth else {
            return Ok(None);
        };
        let Some(access_token) = oauth
            .access_token
            .filter(|t| !t.expose_secret().trim().is_empty())
        else {
            return Ok(None);
        };
        let plan = plan_from_claude(
            oauth.subscription_type.as_deref(),
            oauth.rate_limit_tier.as_deref(),
        );
        let expires_at = oauth
            .expires_at
            .as_ref()
            .and_then(serde_json::Value::as_i64)
            .and_then(|ms| Timestamp::from_millisecond(ms).ok());
        Ok(Some(Self {
            access_token,
            expires_at,
            account: AccountInfo {
                email: None,
                organization: None,
                plan,
                rate_limit_tier: oauth.rate_limit_tier,
            },
        }))
    }

    /// Reads the first source of `location` that holds credentials. Blocking (Keychain and file
    /// reads), so call from a blocking context. The Keychain never prompts.
    ///
    /// A credentials file must be a regular file that only its owner may write
    /// ([`CredentialsError::Insecure`]).
    ///
    /// A source that fails does not hide a later one that works; when nothing works, the first
    /// failure is returned, otherwise `Ok(None)` (signed out).
    ///
    /// # Errors
    /// The first [`CredentialsError`] when no source yielded credentials and one failed.
    pub fn load(
        location: &CredentialLocation,
        secrets: &dyn SecretStore,
    ) -> Result<Option<Self>, CredentialsError> {
        Self::load_with(location, secrets, Prompt::Never)
    }

    /// As [`ClaudeCredentials::load`], but a Keychain source may show macOS's consent prompt
    /// and waits for the user ([`SecretStore::get_interactive`]). Only for an explicit user
    /// request.
    ///
    /// # Errors
    /// As [`ClaudeCredentials::load`].
    pub fn load_interactive(
        location: &CredentialLocation,
        secrets: &dyn SecretStore,
    ) -> Result<Option<Self>, CredentialsError> {
        Self::load_with(location, secrets, Prompt::Allowed)
    }

    fn load_with(
        location: &CredentialLocation,
        secrets: &dyn SecretStore,
        prompt: Prompt,
    ) -> Result<Option<Self>, CredentialsError> {
        let mut first_error = None;
        for source in location.sources() {
            match Self::read_source(source, secrets, prompt) {
                Ok(Some(credentials)) => return Ok(Some(credentials)),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "credential source unreadable");
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(None), Err)
    }

    fn read_source(
        source: &CredentialSource,
        secrets: &dyn SecretStore,
        prompt: Prompt,
    ) -> Result<Option<Self>, CredentialsError> {
        match source {
            CredentialSource::Keychain { service } => {
                let secret = match prompt {
                    Prompt::Never => secrets.get(service)?,
                    Prompt::Allowed => secrets.get_interactive(service)?,
                };
                secret.map_or(Ok(None), |text| Self::parse(text.expose_secret()))
            }
            CredentialSource::File { path } => match read_credentials_file(path)? {
                Some(text) => Self::parse(text.expose_secret()),
                None => Ok(None),
            },
        }
    }

    /// When the access token expires, if the credentials say.
    #[must_use]
    pub fn expires_at(&self) -> Option<Timestamp> {
        self.expires_at
    }

    /// Whether the token is past its `expiresAt` at `now`. Unknown expiry is not expired.
    #[must_use]
    pub fn is_expired(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }

    /// The bearer token.
    #[must_use]
    pub fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    /// Plan and tier from the credentials; email and organization come from the profile
    /// endpoint.
    #[must_use]
    pub fn account(&self) -> &AccountInfo {
        &self.account
    }
}

/// Reads a credentials file, `None` when it does not exist. The checks run on the opened file
/// (a symlink is followed), so they hold for the bytes actually read.
fn read_credentials_file(path: &Utf8Path) -> Result<Option<SecretString>, CredentialsError> {
    let io = |source| CredentialsError::Io {
        path: path.to_owned(),
        source,
    };
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let meta = file.metadata().map_err(io)?;
    let refuse = |reason| CredentialsError::Insecure {
        path: path.to_owned(),
        reason,
    };
    if !meta.is_file() {
        return Err(refuse("not a regular file"));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(refuse("other users may write it"));
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(io)?;
    Ok(Some(SecretString::from(text)))
}
