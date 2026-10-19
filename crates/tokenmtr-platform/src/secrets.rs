//! Read-only credential lookup.

use secrecy::SecretString;

/// Failure to read a secret. A missing secret is not a failure: it is `Ok(None)`.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The OS secret store refused or failed (locked, access denied, ...).
    #[error("secret store error: {0}")]
    Store(String),
    /// Reading the secret needs the user's consent, which a non-interactive lookup never asks
    /// for: the item's access list does not include this app, or the user denied it.
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
    /// Never shows an OS consent prompt: a secret that needs one is [`SecretError::Denied`].
    /// May block; call from a blocking context.
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
    use std::sync::Mutex;

    use super::{SecretError, SecretStore};
    use secrecy::SecretString;
    use secrecy::zeroize::Zeroizing;
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit, SearchResult};
    use security_framework::os::macos::keychain::SecKeychain;

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

    /// Reads generic-password items from the login keychain. Never writes; prompts only through
    /// [`SecretStore::get_interactive`].
    ///
    /// Access is granted per code signature. [`SecretStore::get`] is non-interactive, so an item
    /// another app created is readable only once the user granted this program access: "Always
    /// Allow" at an interactive lookup's prompt, or Keychain Access → the item → Access Control.
    /// Until then it is [`SecretError::Denied`]. The grant names the signature, so it survives a
    /// rebuild only when the build is signed with the same identity.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct KeychainSecrets;

    impl SecretStore for KeychainSecrets {
        fn get(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
            lookup(service, false)
        }

        fn get_interactive(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
            lookup(service, true)
        }
    }

    /// Reads `service`'s item; `interactive` lets macOS show its consent prompt.
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
        use security_framework::passwords::{delete_generic_password, set_generic_password};

        /// Deletes the test item even if an assertion fails.
        struct Cleanup<'a>(&'a str, &'a str);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = delete_generic_password(self.0, self.1);
            }
        }

        #[test]
        fn reads_an_item_created_by_the_test() {
            let service = format!("dev.tokenmtr.test.{}", std::process::id());
            let account = "tokenmtr-test";
            let _cleanup = Cleanup(&service, account);
            set_generic_password(&service, account, b"s3cret-token").unwrap();

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
