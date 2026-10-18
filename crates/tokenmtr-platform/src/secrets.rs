//! Read-only credential lookup.

use secrecy::SecretString;

/// Failure to read a secret. A missing secret is not a failure: it is `Ok(None)`.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The OS secret store refused or failed (locked, access denied, ...).
    #[error("secret store error: {0}")]
    Store(String),
    /// The stored secret is not UTF-8 text.
    #[error("secret is not valid UTF-8")]
    NotUtf8,
}

/// A read-only source of secrets, looked up by service name.
pub trait SecretStore {
    /// Returns the secret stored for `service`, or `None` if there is none.
    ///
    /// May block, and may show an OS consent prompt on first access; call from a blocking
    /// context.
    ///
    /// # Errors
    /// [`SecretError`] if the store cannot be read or the secret is not text.
    fn get(&self, service: &str) -> Result<Option<SecretString>, SecretError>;
}

#[cfg(target_os = "macos")]
pub use keychain::KeychainSecrets;

#[cfg(target_os = "macos")]
mod keychain {
    use super::{SecretError, SecretStore};
    use secrecy::SecretString;
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit, SearchResult};

    /// `errSecItemNotFound` from `<Security/SecBase.h>`.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    /// Reads generic-password items from the login keychain. Never writes.
    ///
    /// Access is granted per code signature (D51): an unsigned rebuild is prompted again.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct KeychainSecrets;

    impl SecretStore for KeychainSecrets {
        fn get(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
            let found = ItemSearchOptions::new()
                .class(ItemClass::generic_password())
                .service(service)
                .load_data(true)
                .limit(Limit::Max(1))
                .search();
            let results = match found {
                Ok(results) => results,
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(None),
                Err(e) => return Err(SecretError::Store(e.to_string())),
            };
            results
                .into_iter()
                .find_map(|result| match result {
                    SearchResult::Data(bytes) => Some(bytes),
                    _ => None,
                })
                .map(|bytes| {
                    String::from_utf8(bytes)
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
