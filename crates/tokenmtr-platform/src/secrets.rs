//! Read-only credential lookup.

use secrecy::SecretString;

/// Failure to read a secret. A missing secret is not a failure: it is `Ok(None)`.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The OS secret store refused or failed (locked, access denied, ...).
    #[error("secret store error: {0}")]
    Store(String),
    /// Reading the secret needs the user's consent, which a non-interactive lookup never waits
    /// for: the item's access list does not include the reader, or the user denied it.
    #[error("access to the secret was not granted")]
    Denied,
    /// The stored secret is not UTF-8 text.
    #[error("secret is not valid UTF-8")]
    NotUtf8,
}

/// A read-only source of secrets, looked up by service name.
pub trait SecretStore {
    /// Returns the secret stored for `service`, or `None` if there is none.
    ///
    /// Never waits for the user: a secret that needs consent is [`SecretError::Denied`]. May
    /// block briefly; call from a blocking context.
    ///
    /// # Errors
    /// [`SecretError`] if the store cannot be read or the secret is not text.
    fn get(&self, service: &str) -> Result<Option<SecretString>, SecretError>;

    /// As [`get`](Self::get), but lets the OS show its consent prompt and waits for the user's
    /// answer, so the user can grant this program access for good ("Always Allow"). Only for
    /// an explicit user request: a background caller must never block on a prompt. A refused
    /// prompt is [`SecretError::Denied`]. The default never prompts.
    ///
    /// # Errors
    /// As [`get`](Self::get).
    fn get_interactive(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
        self.get(service)
    }
}

/// For an OS whose agents keep no secret in the OS store (Linux: Claude Code uses only
/// `~/.claude/.credentials.json`, which callers read themselves): every lookup finds nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _service: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
pub use keychain::KeychainSecrets;

#[cfg(target_os = "macos")]
mod keychain {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::{SecretError, SecretStore};
    use secrecy::SecretString;
    use secrecy::zeroize::Zeroizing;
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit, SearchResult};
    use security_framework::os::macos::keychain::SecKeychain;

    /// Apple's Keychain tool. Claude Code writes its item through it, so the item's access list
    /// and partition list always trust it.
    const SECURITY: &str = "/usr/bin/security";
    /// A `security` read that takes longer is waiting on a consent prompt: it is killed.
    const SECURITY_TIMEOUT: Duration = Duration::from_secs(10);
    /// How often a running `security` read is checked.
    const SECURITY_POLL: Duration = Duration::from_millis(10);
    /// `security`'s exit status is the low byte of the `OSStatus`: `errSecItemNotFound`.
    const EXIT_ITEM_NOT_FOUND: i32 = 44;
    /// `errSecInteractionNotAllowed`, `errSecAuthFailed`, `errSecUserCanceled`, as exit statuses.
    const EXIT_DENIED: [i32; 3] = [36, 51, 128];

    /// `errSecItemNotFound` from `<Security/SecBase.h>`.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    /// `errSecInteractionNotAllowed`: the lookup needed a consent prompt, which is disabled.
    const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
    /// `errSecAuthFailed`: the user or the item's access list refused.
    const ERR_SEC_AUTH_FAILED: i32 = -25293;
    /// `errSecUserCanceled`.
    const ERR_SEC_USER_CANCELED: i32 = -128;

    /// Serializes lookups: disabling Keychain UI is process-wide, so two overlapping lookups
    /// would re-enable it under each other, or disable it under an interactive one.
    static NO_UI: Mutex<()> = Mutex::new(());

    /// Reads generic-password items from the login keychain. Never writes; waits on a prompt
    /// only through [`SecretStore::get_interactive`].
    ///
    /// [`SecretStore::get`] reads through the Keychain API with prompts disabled, which works
    /// once the user granted this program access ("Always Allow" at an interactive lookup's
    /// prompt, or Keychain Access → the item → Access Control). The grant names the code
    /// signature, so it survives a rebuild only when signed with the same identity.
    ///
    /// Claude Code rewrites its credentials item through `security` at each token refresh,
    /// which drops that grant but keeps trusting `security`: a refused read falls back on
    /// `security find-generic-password -w`. An item that trusts neither makes `security` show
    /// a consent prompt: the read is killed after [`SECURITY_TIMEOUT`] and is
    /// [`SecretError::Denied`], so the caller must stop reading that item.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct KeychainSecrets;

    impl SecretStore for KeychainSecrets {
        fn get(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
            match lookup(service, false) {
                Err(SecretError::Denied) => security_lookup(service),
                found => found,
            }
        }

        fn get_interactive(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
            lookup(service, true)
        }
    }

    /// Reads `service`'s item with `security`, killing it after [`SECURITY_TIMEOUT`].
    fn security_lookup(service: &str) -> Result<Option<SecretString>, SecretError> {
        let store = |e: std::io::Error| SecretError::Store(format!("{SECURITY}: {e}"));
        let mut child = Command::new(SECURITY)
            .args(["find-generic-password", "-s", service, "-w"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(store)?;
        let deadline = Instant::now() + SECURITY_TIMEOUT;
        // The secret is far smaller than the pipe buffer: the child never blocks on stdout.
        let status = loop {
            if let Some(status) = child.try_wait().map_err(store)? {
                break status;
            }
            if Instant::now() >= deadline {
                // Best effort: the read is refused whether or not the kill lands.
                let _ = child.kill();
                let _ = child.wait();
                return Err(SecretError::Denied);
            }
            std::thread::sleep(SECURITY_POLL);
        };
        match status.code() {
            Some(0) => {}
            Some(EXIT_ITEM_NOT_FOUND) => return Ok(None),
            Some(code) if EXIT_DENIED.contains(&code) => return Err(SecretError::Denied),
            _ => return Err(SecretError::Store(format!("{SECURITY} failed: {status}"))),
        }
        let mut bytes = Zeroizing::new(Vec::new());
        if let Some(mut stdout) = child.stdout.take() {
            stdout.read_to_end(&mut bytes).map_err(store)?;
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        std::str::from_utf8(&bytes)
            .map(|text| Some(SecretString::from(text)))
            .map_err(|_| SecretError::NotUtf8)
    }

    /// Reads `service`'s item through the Keychain API; `interactive` lets macOS show its
    /// consent prompt.
    fn lookup(service: &str, interactive: bool) -> Result<Option<SecretString>, SecretError> {
        let found = {
            let _serial = NO_UI
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _no_ui = if interactive {
                None
            } else {
                Some(
                    SecKeychain::disable_user_interaction()
                        .map_err(|e| SecretError::Store(e.to_string()))?,
                )
            };
            ItemSearchOptions::new()
                .class(ItemClass::generic_password())
                .service(service)
                .load_data(true)
                .limit(Limit::Max(1))
                .search()
        };
        let results = match found {
            Ok(results) => results,
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(None),
            Err(e)
                if [
                    ERR_SEC_INTERACTION_NOT_ALLOWED,
                    ERR_SEC_AUTH_FAILED,
                    ERR_SEC_USER_CANCELED,
                ]
                .contains(&e.code()) =>
            {
                return Err(SecretError::Denied);
            }
            Err(e) => return Err(SecretError::Store(e.to_string())),
        };
        results
            .into_iter()
            .find_map(|result| match result {
                SearchResult::Data(bytes) => Some(Zeroizing::new(bytes)),
                _ => None,
            })
            .map(|bytes| {
                std::str::from_utf8(&bytes)
                    .map(SecretString::from)
                    .map_err(|_| SecretError::NotUtf8)
            })
            .transpose()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use secrecy::ExposeSecret;

        /// Runs `security` with `args`, as Claude Code does to write its item.
        fn security(args: &[&str]) -> bool {
            Command::new(SECURITY)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }

        /// Deletes the test item even if an assertion fails.
        struct Cleanup<'a>(&'a str);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                security(&["delete-generic-password", "-s", self.0]);
            }
        }

        #[test]
        fn reads_an_item_written_by_security() {
            let service = format!("dev.tokenmtr.test.{}", std::process::id());
            let _cleanup = Cleanup(&service);
            let written = security(&[
                "add-generic-password",
                "-U",
                "-a",
                "tokenmtr-test",
                "-s",
                &service,
                "-w",
                "s3cret-token",
            ]);
            assert!(written, "cannot write the test item");

            let secret = KeychainSecrets.get(&service).unwrap().unwrap();
            assert_eq!(secret.expose_secret(), "s3cret-token");
        }

        #[test]
        fn reads_an_item_written_by_this_program() {
            let service = format!("dev.tokenmtr.test.self.{}", std::process::id());
            let _cleanup = Cleanup(&service);
            security_framework::passwords::set_generic_password(
                &service,
                "tokenmtr-test",
                b"s3cret-token",
            )
            .unwrap();

            let secret = KeychainSecrets.get(&service).unwrap().unwrap();
            assert_eq!(secret.expose_secret(), "s3cret-token");
        }

        #[test]
        fn missing_item_is_none() {
            let service = format!("dev.tokenmtr.test.missing.{}", std::process::id());
            assert!(KeychainSecrets.get(&service).unwrap().is_none());
        }
    }
}

#[cfg(test)]
mod no_secrets_tests {
    use super::*;

    #[test]
    fn no_secrets_finds_nothing() {
        assert!(NoSecrets.get("Claude Code-credentials").unwrap().is_none());
        assert!(NoSecrets.get_interactive("x").unwrap().is_none());
    }
}
