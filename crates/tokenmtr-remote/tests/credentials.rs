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
            Some(Err(())) => Err(SecretError::Store("access denied".to_owned())),
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

    assert!(matches!(result, Err(CredentialsError::Secret(_))));
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
