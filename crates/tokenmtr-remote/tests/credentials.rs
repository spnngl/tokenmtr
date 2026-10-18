//! Credential lookup order, plan detection and Keychain naming.

mod common;

use std::collections::HashMap;

use camino::Utf8PathBuf;
use common::{FAKE_TOKEN, TestResult, fixture_text};
use secrecy::{ExposeSecret, SecretString};
use tokenmtr_core::Plan;
use tokenmtr_platform::{SecretError, SecretStore};
use tokenmtr_remote::{
    ClaudeCredentials, CredentialLocation, CredentialSource, CredentialsError,
    DEFAULT_KEYCHAIN_SERVICE, keychain_service_for_config_dir,
};

/// An in-memory secret store; a service mapped to `Err` simulates a denied Keychain.
#[derive(Default)]
struct FakeStore(HashMap<String, Result<String, ()>>);

impl FakeStore {
    fn with(mut self, service: &str, value: Result<String, ()>) -> Self {
        self.0.insert(service.to_owned(), value);
        self
    }
}

impl SecretStore for FakeStore {
    fn get(&self, service: &str) -> Result<Option<SecretString>, SecretError> {
        match self.0.get(service) {
            None => Ok(None),
            Some(Ok(text)) => Ok(Some(SecretString::from(text.clone()))),
            Some(Err(())) => Err(SecretError::Denied),
        }
    }
}

fn token_json(token: &str) -> String {
    format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}","subscriptionType":"pro"}}}}"#)
}

fn temp_home() -> Result<(tempfile::TempDir, Utf8PathBuf), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
        .map_err(|p| format!("non-UTF-8 temp dir {}", p.display()))?;
    Ok((dir, path))
}

fn token_of(credentials: Option<ClaudeCredentials>) -> Result<String, Box<dyn std::error::Error>> {
    let credentials = credentials.ok_or("expected credentials")?;
    Ok(credentials.access_token().expose_secret().to_owned())
}

#[test]
fn parses_token_plan_and_tier_from_the_fixture() -> TestResult {
    let credentials = ClaudeCredentials::parse(&fixture_text("claude_credentials.json")?)?
        .ok_or("expected credentials")?;

    assert_eq!(credentials.access_token().expose_secret(), FAKE_TOKEN);
    assert_eq!(credentials.account().plan, Some(Plan::Max20x));
    assert_eq!(
        credentials.account().rate_limit_tier.as_deref(),
        Some("default_claude_max_20x")
    );
    assert_eq!(credentials.account().email, None);
    Ok(())
}

#[test]
fn documents_without_a_token_mean_signed_out() -> TestResult {
    for json in [
        "{}",
        r#"{"mcpOAuth":{}}"#,
        r#"{"claudeAiOauth":{}}"#,
        r#"{"claudeAiOauth":{"accessToken":"  "}}"#,
    ] {
        assert!(ClaudeCredentials::parse(json)?.is_none(), "{json}");
    }
    Ok(())
}

#[test]
fn default_profile_prefers_keychain_over_file() -> TestResult {
    let (_dir, home) = temp_home()?;
    std::fs::create_dir_all(home.join(".claude"))?;
    std::fs::write(
        home.join(".claude/.credentials.json"),
        token_json("from-file"),
    )?;
    let store =
        FakeStore::default().with(DEFAULT_KEYCHAIN_SERVICE, Ok(token_json("from-keychain")));

    let credentials = ClaudeCredentials::load(&CredentialLocation::default_profile(&home), &store)?;

    assert_eq!(token_of(credentials)?, "from-keychain");
    Ok(())
}

#[test]
fn default_profile_falls_back_to_the_file() -> TestResult {
    let (_dir, home) = temp_home()?;
    std::fs::create_dir_all(home.join(".claude"))?;
    std::fs::write(
        home.join(".claude/.credentials.json"),
        token_json("from-file"),
    )?;

    let credentials = ClaudeCredentials::load(
        &CredentialLocation::default_profile(&home),
        &FakeStore::default(),
    )?;

    assert_eq!(token_of(credentials)?, "from-file");
    Ok(())
}

#[test]
fn other_profiles_prefer_their_file_then_the_derived_keychain_item() -> TestResult {
    let (_dir, config_dir) = temp_home()?;
    let service = keychain_service_for_config_dir(&config_dir);
    let store = FakeStore::default().with(&service, Ok(token_json("from-keychain")));
    let location = CredentialLocation::config_dir(&config_dir);

    // No file yet: the derived Keychain item answers.
    assert_eq!(
        token_of(ClaudeCredentials::load(&location, &store)?)?,
        "from-keychain"
    );

    // The file wins once present.
    std::fs::write(
        config_dir.join(".credentials.json"),
        token_json("from-file"),
    )?;
    assert_eq!(
        token_of(ClaudeCredentials::load(&location, &store)?)?,
        "from-file"
    );
    Ok(())
}

#[test]
fn explicit_keychain_service_overrides_the_derived_name() {
    let location = CredentialLocation::config_dir(&Utf8PathBuf::from("/x/.claude-work"))
        .with_keychain_service("Custom-service");

    assert!(location.sources().iter().any(
        |s| matches!(s, CredentialSource::Keychain { service } if service == "Custom-service")
    ));
}

#[test]
fn keychain_name_for_a_config_dir_is_pinned() {
    // sha256("/Users/test/.claude-work") = 03abf0ee6e60...; UNVERIFIED against a real Claude
    // Code install (see `keychain_service_for_config_dir`).
    assert_eq!(
        keychain_service_for_config_dir(&Utf8PathBuf::from("/Users/test/.claude-work")),
        "Claude Code-credentials-03abf0ee"
    );
}

#[test]
fn nothing_anywhere_is_signed_out() -> TestResult {
    let (_dir, home) = temp_home()?;

    let credentials = ClaudeCredentials::load(
        &CredentialLocation::default_profile(&home),
        &FakeStore::default(),
    )?;

    assert!(credentials.is_none());
    Ok(())
}

#[test]
fn a_denied_keychain_does_not_hide_a_working_file() -> TestResult {
    let (_dir, home) = temp_home()?;
    std::fs::create_dir_all(home.join(".claude"))?;
    std::fs::write(
        home.join(".claude/.credentials.json"),
        token_json("from-file"),
    )?;
    let store = FakeStore::default().with(DEFAULT_KEYCHAIN_SERVICE, Err(()));

    let credentials = ClaudeCredentials::load(&CredentialLocation::default_profile(&home), &store)?;

    assert_eq!(token_of(credentials)?, "from-file");
    Ok(())
}

#[test]
fn a_denied_keychain_with_no_file_is_an_error() -> TestResult {
    let (_dir, home) = temp_home()?;
    let store = FakeStore::default().with(DEFAULT_KEYCHAIN_SERVICE, Err(()));

    let result = ClaudeCredentials::load(&CredentialLocation::default_profile(&home), &store);

    let error = result.err().ok_or("expected an error")?;
    assert!(error.is_keychain_denied());

    // Once denied, the daemon asks the file sources only.
    let location = CredentialLocation::default_profile(&home).without_keychain();
    assert!(
        location
            .sources()
            .iter()
            .all(|s| matches!(s, CredentialSource::File { .. }))
    );
    assert!(ClaudeCredentials::load(&location, &store)?.is_none());
    Ok(())
}

#[test]
fn expiry_comes_from_expires_at() -> TestResult {
    let fixture = ClaudeCredentials::parse(&fixture_text("claude_credentials.json")?)?
        .ok_or("expected credentials")?;
    assert_eq!(
        fixture.expires_at(),
        Some(jiff::Timestamp::from_millisecond(1_893_456_000_000)?)
    );
    assert!(!fixture.is_expired("2029-12-31T23:59:59Z".parse()?));
    assert!(fixture.is_expired("2030-01-01T00:00:00Z".parse()?));

    for json in [
        r#"{"claudeAiOauth":{"accessToken":"t"}}"#,
        r#"{"claudeAiOauth":{"accessToken":"t","expiresAt":"soon"}}"#,
    ] {
        let credentials = ClaudeCredentials::parse(json)?.ok_or("expected credentials")?;
        assert_eq!(credentials.expires_at(), None, "{json}");
        assert!(!credentials.is_expired(jiff::Timestamp::MAX));
    }
    Ok(())
}

#[test]
fn a_file_other_users_may_write_is_refused() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, config_dir) = temp_home()?;
    let file = config_dir.join(".credentials.json");
    std::fs::write(&file, token_json("from-file"))?;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666))?;
    let location = CredentialLocation::config_dir(&config_dir);

    let result = ClaudeCredentials::load(&location, &FakeStore::default());
    assert!(matches!(result, Err(CredentialsError::Insecure { .. })));

    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    assert_eq!(
        token_of(ClaudeCredentials::load(&location, &FakeStore::default())?)?,
        "from-file"
    );
    Ok(())
}

#[test]
fn a_symlinked_file_is_checked_at_its_target() -> TestResult {
    let (_dir, config_dir) = temp_home()?;
    std::fs::write(config_dir.join("real.json"), token_json("via-link"))?;
    std::os::unix::fs::symlink("real.json", config_dir.join(".credentials.json"))?;
    std::fs::create_dir(config_dir.join("dir"))?;
    let location = CredentialLocation::config_dir(&config_dir);

    assert_eq!(
        token_of(ClaudeCredentials::load(&location, &FakeStore::default())?)?,
        "via-link"
    );

    std::fs::remove_file(config_dir.join(".credentials.json"))?;
    std::os::unix::fs::symlink("dir", config_dir.join(".credentials.json"))?;
    let result = ClaudeCredentials::load(&location, &FakeStore::default());
    assert!(matches!(result, Err(CredentialsError::Insecure { .. })));
    Ok(())
}

#[test]
fn a_corrupt_file_is_malformed() -> TestResult {
    let (_dir, config_dir) = temp_home()?;
    std::fs::write(config_dir.join(".credentials.json"), "{ not json")?;

    let result = ClaudeCredentials::load(
        &CredentialLocation::config_dir(&config_dir),
        &FakeStore::default(),
    );

    assert!(matches!(result, Err(CredentialsError::Malformed)));
    Ok(())
}
