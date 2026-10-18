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
    /// would re-enable it under each other.
    static NO_UI: Mutex<()> = Mutex::new(());

    /// Reads generic-password items from the login keychain. Never writes, never prompts.
    ///
    /// Access is granted per code signature (D51). Since the lookup never shows the "Always
    /// Allow" prompt, an item another app created is readable only once the user adds this app
    /// to the item's access list in Keychain Access; until then it is [`SecretError::Denied`].
    #[derive(Debug, Default, Clone, Copy)]
    pub struct KeychainSecrets;

    impl SecretStore for KeychainSecrets {
        fn get(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
            let found = {
                let _serial = NO_UI
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _no_ui = SecKeychain::disable_user_interaction()
                    .map_err(|e| SecretError::Store(e.to_string()))?;
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
