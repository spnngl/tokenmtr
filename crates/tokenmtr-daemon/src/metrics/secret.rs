//! Reading the credentials of a metrics output when the output starts (P14).
//!
//! Every error message names the source (the variable, the file) and never shows its content.

use camino::Utf8Path;
use secrecy::SecretString;
use tokenmtr_config::{Auth, Env, SecretSource};
use tokenmtr_remote::Credential;

/// The credential `auth` describes, with `*_env` and `*_file` read now.
///
/// # Errors
/// A readable description of what is wrong with the source: an unset variable, a missing, empty,
/// unreadable or too-open file.
pub(crate) fn credential(env: &dyn Env, auth: &Auth) -> Result<Credential, String> {
    match auth {
        Auth::Basic { username, password } => Ok(Credential::Basic {
            username: username.clone(),
            password: secret(env, password)?,
        }),
        Auth::Bearer { token } => Ok(Credential::Bearer(secret(env, token)?)),
    }
}

fn secret(env: &dyn Env, source: &SecretSource) -> Result<SecretString, String> {
    match source {
        SecretSource::Inline(secret) => Ok(secret.clone()),
        SecretSource::Env(name) => {
            let value = env
                .var(name)
                .ok_or_else(|| format!("secret env {name} is unset"))?;
            non_empty(value, &format!("secret env {name}"))
        }
        SecretSource::File(path) => from_file(path),
    }
}

/// No token or password is this long; a bigger file is not a secret file.
const MAX_SECRET_BYTES: u64 = 16 * 1024;

fn from_file(path: &Utf8Path) -> Result<SecretString, String> {
    use std::io::Read as _;
    let name = format!("secret file {path}");
    // One handle for the checks and the read, so the file cannot change in between.
    let file = std::fs::File::open(path).map_err(|e| format!("{name}: {}", e.kind()))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("{name}: {}", e.kind()))?;
    if !metadata.is_file() {
        return Err(format!("{name} is not a regular file"));
    }
    refuse_open_permissions(&metadata, &name)?;
    let mut text = String::new();
    file.take(MAX_SECRET_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{name}: {}", e.kind()))?;
    if u64::try_from(text.len()).is_ok_and(|bytes| bytes > MAX_SECRET_BYTES) {
        return Err(format!("{name} is larger than {MAX_SECRET_BYTES} bytes"));
    }
    non_empty(text, &name)
}

/// Like `ssh` for a private key: a file readable by group or others is refused.
#[cfg(unix)]
fn refuse_open_permissions(metadata: &std::fs::Metadata, name: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = metadata.permissions().mode() & 0o777;
    if mode.trailing_zeros() >= 6 {
        Ok(())
    } else {
        Err(format!(
            "{name} is readable by others (mode {mode:04o}); run chmod 600"
        ))
    }
}

#[cfg(not(unix))]
fn refuse_open_permissions(_: &std::fs::Metadata, _: &str) -> Result<(), String> {
    Ok(())
}

/// Trims one trailing newline, then refuses an empty or multi-line value.
fn non_empty(mut value: String, name: &str) -> Result<SecretString, String> {
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    if value.trim().is_empty() {
        return Err(format!("{name} is empty"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} has a control character"));
    }
    Ok(SecretString::from(value))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use camino::Utf8PathBuf;
    use secrecy::ExposeSecret as _;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn bearer(source: SecretSource) -> Auth {
        Auth::Bearer { token: source }
    }

    fn token(credential: &Credential) -> &str {
        match credential {
            Credential::Bearer(token) => token.expose_secret(),
            Credential::Basic { password, .. } => password.expose_secret(),
        }
    }

    fn file(dir: &tempfile::TempDir, text: &str, mode: u32) -> Result<Utf8PathBuf, std::io::Error> {
        let path = dir.path().join("secret");
        std::fs::write(&path, text)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
        Ok(Utf8PathBuf::from_path_buf(path).unwrap_or_default())
    }

    #[test]
    fn an_env_secret_is_read_when_set_and_fails_when_not() -> TestResult {
        let env = |key: &str| (key == "SET").then(|| "hunter2".to_owned());
        let ok = credential(&env, &bearer(SecretSource::Env("SET".to_owned())))?;
        assert_eq!(token(&ok), "hunter2");
        let error = credential(&env, &bearer(SecretSource::Env("NOPE".to_owned())))
            .err()
            .ok_or("must fail")?;
        assert_eq!(error, "secret env NOPE is unset");
        Ok(())
    }

    #[test]
    fn a_file_secret_loses_one_trailing_newline() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = file(&dir, "tok\n\n", 0o600)?;
        // One newline goes; a second one is a control character, not a secret.
        let error = credential(&|_: &str| None, &bearer(SecretSource::File(path.clone())))
            .err()
            .ok_or("must fail")?;
        assert!(error.contains("control character"), "{error}");
        let path = file(&dir, "tok\r\n", 0o400)?;
        let ok = credential(&|_: &str| None, &bearer(SecretSource::File(path)))?;
        assert_eq!(token(&ok), "tok");
        Ok(())
    }

    #[test]
    fn bad_files_fail_without_showing_their_content() -> TestResult {
        let dir = tempfile::tempdir()?;
        let open = file(&dir, "topsecret", 0o644)?;
        let other = tempfile::tempdir()?;
        let empty = file(&other, "\n", 0o600)?;
        let missing = Utf8PathBuf::from("/no/such/secret");
        for (source, expect) in [
            (open, "readable by others (mode 0644)"),
            (empty, "is empty"),
            (missing, "not found"),
        ] {
            let error = credential(&|_: &str| None, &bearer(SecretSource::File(source)))
                .err()
                .ok_or("must fail")?;
            assert!(error.contains(expect), "{error}");
            assert!(!error.contains("topsecret"), "{error}");
        }
        Ok(())
    }

    #[test]
    fn basic_auth_keeps_the_username() -> TestResult {
        let auth = Auth::Basic {
            username: "u".to_owned(),
            password: SecretSource::Inline(SecretString::from("p")),
        };
        let Credential::Basic { username, password } = credential(&|_: &str| None, &auth)? else {
            return Err("expected basic".into());
        };
        assert_eq!((username.as_str(), password.expose_secret()), ("u", "p"));
        Ok(())
    }
}
