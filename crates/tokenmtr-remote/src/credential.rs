//! A user-supplied secret for a metrics endpoint: sent on a push, required on a scrape.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use secrecy::{ExposeSecret as _, SecretString};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

/// How a push authenticates to its receiver, or how a scraper must authenticate to an exporter.
///
/// Secrets are [`SecretString`]: `Debug` prints `[REDACTED]`, and nothing here is logged.
#[derive(Debug, Clone)]
pub enum Credential {
    /// HTTP basic authentication.
    Basic {
        /// The user name. It cannot contain `:`.
        username: String,
        /// The password.
        password: SecretString,
    },
    /// A bearer token.
    Bearer(SecretString),
}

impl Credential {
    /// The `Authorization` header value this credential stands for.
    pub(crate) fn header(&self) -> SecretString {
        match self {
            Self::Basic { username, password } => {
                let pair = format!("{username}:{}", password.expose_secret());
                SecretString::from(format!("Basic {}", STANDARD.encode(pair)))
            }
            Self::Bearer(token) => SecretString::from(format!("Bearer {}", token.expose_secret())),
        }
    }

    /// The scheme of the `Authorization` header (RFC 7235: matched without regard to case).
    const fn scheme(&self) -> &'static str {
        match self {
            Self::Basic { .. } => "Basic",
            Self::Bearer(_) => "Bearer",
        }
    }

    /// The `WWW-Authenticate` challenge of a scrape that failed authentication.
    pub(crate) const fn challenge(&self) -> &'static str {
        match self {
            Self::Basic { .. } => "Basic realm=\"tokenmtr\", charset=\"UTF-8\"",
            Self::Bearer(_) => "Bearer realm=\"tokenmtr\"",
        }
    }
}

/// The digest of the `Authorization` value an exporter accepts.
///
/// Both sides are hashed before the constant-time comparison, so neither the value's content
/// nor its length leaks through timing.
#[derive(Clone)]
pub(crate) struct Expected {
    scheme: &'static str,
    digest: [u8; 32],
}

impl std::fmt::Debug for Expected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Expected([REDACTED])")
    }
}

impl Expected {
    pub(crate) fn of(credential: &Credential) -> Self {
        Self {
            scheme: credential.scheme(),
            digest: digest(credential.header().expose_secret()),
        }
    }

    /// Whether `presented` (the request's `Authorization` header, if any) matches.
    pub(crate) fn accepts(&self, presented: Option<&[u8]>) -> bool {
        let presented = presented.unwrap_or_default();
        // `basic` and `BASIC` are the same scheme; the credentials after it are compared exactly.
        let canonical = match presented.iter().position(|byte| *byte == b' ') {
            Some(space) if presented[..space].eq_ignore_ascii_case(self.scheme.as_bytes()) => {
                [self.scheme.as_bytes(), &presented[space..]].concat()
            }
            _ => presented.to_vec(),
        };
        let presented = digest_bytes(&canonical);
        // A missing header hashes like an empty one, which no credential produces.
        bool::from(self.digest.ct_eq(&presented)) && presented != digest_bytes(b"")
    }
}

fn digest(text: &str) -> [u8; 32] {
    digest_bytes(text.as_bytes())
}

fn digest_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic() -> Credential {
        Credential::Basic {
            username: "u".to_owned(),
            password: SecretString::from("p"),
        }
    }

    #[test]
    fn headers_follow_rfc_7617_and_6750() {
        assert_eq!(basic().header().expose_secret(), "Basic dTpw");
        let bearer = Credential::Bearer(SecretString::from("tok"));
        assert_eq!(bearer.header().expose_secret(), "Bearer tok");
    }

    #[test]
    fn only_the_exact_header_is_accepted() {
        let expected = Expected::of(&basic());
        assert!(expected.accepts(Some(b"Basic dTpw")));
        assert!(!expected.accepts(Some(b"Basic dTpx")));
        assert!(expected.accepts(Some(b"basic dTpw")));
        assert!(expected.accepts(Some(b"BASIC dTpw")));
        assert!(!expected.accepts(Some(b"basic dtpw")));
        assert!(!expected.accepts(Some(b"Bearer dTpw")));
        assert!(!expected.accepts(Some(b"dTpw")));
        assert!(!expected.accepts(Some(b"")));
        assert!(!expected.accepts(None));
    }

    #[test]
    fn debug_never_shows_a_secret() {
        let shown = format!(
            "{:?} {:?} {:?}",
            basic(),
            Credential::Bearer(SecretString::from("tok-secret")),
            Expected::of(&basic())
        );
        assert!(
            !shown.contains("tok-secret") && !shown.contains("\"p\""),
            "{shown}"
        );
    }
}
