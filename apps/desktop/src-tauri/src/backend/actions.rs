//! Actions: everything that changes something (config, files, daemon, service).
//!
//! Plain functions over a [`Backend`], so they run in tests without a window. The daemon is the
//! only writer of the store: when it answers, scans, exports and backups go through the
//! control socket; when it does not, they run in-process like the CLI does.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_app::{EraseScope, ReportRequest};
use tokenmtr_config::{GitName, MenubarRing, RawProject};
use tokenmtr_control::Client;
use tokenmtr_core::{AgentId, ProfileId, ProjectKey, SessionId, SubscriptionSpec};
use tokenmtr_platform::{LaunchdService, SystemRunner};

use super::Backend;
use crate::daemon::{Sidecar, client_info, install_service};
use crate::error::{CommandError, ErrorKind};
use crate::request::RevealTarget;

async fn connect(backend: &Backend) -> Result<Client, CommandError> {
    Ok(Client::connect(backend.transport(), client_info()).await?)
}

/// Whether `error` means "no daemon", the only case that falls back to in-process work.
const fn is_offline(error: &CommandError) -> bool {
    matches!(error.kind, ErrorKind::DaemonOffline)
}

/// Scans every agent now.
///
/// # Errors
///
/// The daemon failed the scan, or the in-process scan did.
pub async fn scan_now(backend: &Backend) -> Result<(), CommandError> {
    match connect(backend).await {
        Ok(mut client) => client.scan_now(None).await.map(drop).map_err(Into::into),
        Err(error) if is_offline(&error) => {
            let backend = backend.clone();
            super::blocking(move || {
                backend.context()?.scan_direct(None, false)?;
                Ok(())
            })
            .await
        }
        Err(error) => Err(error),
    }
}

/// Runs every export sink now. Needs the daemon: sinks are its job.
///
/// # Errors
///
/// No daemon answers, or a sink failed.
pub async fn export_now(backend: &Backend) -> Result<(), CommandError> {
    connect(backend)
        .await?
        .export_now(None)
        .await
        .map(drop)
        .map_err(Into::into)
}

/// Writes a backup now.
///
/// # Errors
///
/// The backup failed.
pub async fn backup_now(backend: &Backend) -> Result<(), CommandError> {
    match connect(backend).await {
        Ok(mut client) => client.backup_now().await.map(drop).map_err(Into::into),
        Err(error) if is_offline(&error) => {
            let backend = backend.clone();
            super::blocking(move || {
                backend.context()?.backup_direct(Timestamp::now())?;
                Ok(())
            })
            .await
        }
        Err(error) => Err(error),
    }
}

/// Asks the daemon to re-read the config; a missing daemon is fine, it reads it at start.
pub async fn reload_daemon(backend: &Backend) {
    if let Ok(mut client) = connect(backend).await
        && let Err(error) = client.reload().await
    {
        tracing::warn!(%error, "daemon refused the config reload");
    }
}

/// Installs the background service, or points it at the bundled binary again and restarts it.
///
/// # Errors
///
/// The sidecar is missing, or launchd refused.
pub fn install_daemon(backend: &Backend) -> Result<(), CommandError> {
    let sidecar = Sidecar::locate()?;
    let service = LaunchdService::new(backend.paths(), SystemRunner);
    Ok(install_service(&service, &sidecar)?)
}

/// The path `target` stands for, if it exists. A missing config file is created first, holding
/// only comments.
///
/// # Errors
///
/// The session has no transcript on disk, the path does not exist, the config file cannot be
/// created, or the store cannot be read.
pub fn reveal_path(backend: &Backend, target: &RevealTarget) -> Result<Utf8PathBuf, CommandError> {
    let transcript = |agent: AgentId, session: &SessionId| {
        super::shell::session_origin(&backend.context()?, agent, session)?
            .transcript_path
            .map(Utf8PathBuf::from)
            .ok_or_else(|| CommandError::invalid(format!("session {session} has no transcript")))
    };
    let path = match target {
        RevealTarget::ConfigFile => {
            tokenmtr_app::ensure_config_file(backend.config_path())?;
            backend.config_path().to_owned()
        }
        RevealTarget::ExportsDir => backend.paths().exports_dir().clone(),
        RevealTarget::Transcript { agent, session } => transcript(*agent, session)?,
        RevealTarget::TranscriptFolder { agent, session } => {
            let file = transcript(*agent, session)?;
            file.parent().map_or(file.clone(), Utf8Path::to_owned)
        }
    };
    if !path.exists() {
        return Err(CommandError::invalid(format!("{path} does not exist")));
    }
    Ok(path)
}

/// The `[[projects]]` rule the app suggests for the working directory `cwd`: named after its
/// repository when git or Superset knows it, else after its last component.
///
/// # Errors
///
/// `cwd` is not an absolute path, or the config cannot be read.
pub fn project_rule_for(
    backend: &Backend,
    cwd: &str,
    now: Timestamp,
) -> Result<RawProject, CommandError> {
    let name = backend
        .context()?
        .suggested_project_name(absolute(cwd)?, now)
        .unwrap_or_else(|| "project".to_owned());
    rule(cwd, &name)
}

fn absolute(cwd: &str) -> Result<&Utf8Path, CommandError> {
    let path = Utf8Path::new(cwd);
    if !path.is_absolute() {
        return Err(CommandError::invalid(format!(
            "{cwd} is not an absolute path"
        )));
    }
    Ok(path)
}

/// The rule naming exactly the directory `cwd` `name`.
fn rule(cwd: &str, name: &str) -> Result<RawProject, CommandError> {
    absolute(cwd)?;
    Ok(RawProject {
        pattern: escape_glob(cwd),
        name: ProjectKey::try_from(name.trim()).map_err(CommandError::invalid)?,
        client: None,
    })
}

/// `text` with every glob metacharacter escaped, so the pattern matches only `text`.
fn escape_glob(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Labels the sessions started in `cwd` with the project `name`.
///
/// # Errors
///
/// An invalid directory, a blank name, or a config that would become invalid (the file stays
/// untouched).
pub fn add_project_rule(backend: &Backend, cwd: &str, name: &str) -> Result<(), CommandError> {
    let rule = rule(cwd, name)?;
    backend.context()?.add_project_rule(&rule)?;
    Ok(())
}

/// Asks the daemon to read the Claude credentials once with the Keychain's consent prompt
/// allowed, so the user can grant it access for good. Needs the daemon: the grant goes to the
/// binary that asks.
///
/// # Errors
///
/// No daemon answers, or the request failed.
pub async fn connect_keychain(backend: &Backend) -> Result<(), CommandError> {
    connect(backend)
        .await?
        .connect_keychain()
        .await
        .map(drop)
        .map_err(Into::into)
}

/// Records a subscription in the config.
///
/// # Errors
///
/// A config that would become invalid, for example overlapping subscriptions.
pub fn add_subscription(backend: &Backend, spec: &SubscriptionSpec) -> Result<(), CommandError> {
    backend.context()?.add_subscription(spec)?;
    Ok(())
}

/// Sets which quota windows the menu bar draws as rings and which profiles it skips.
///
/// # Errors
///
/// A config that would become invalid, for example a ring kind listed twice.
pub fn set_menubar_rings(
    backend: &Backend,
    rings: &[MenubarRing],
    hidden_profiles: &[ProfileId],
) -> Result<(), CommandError> {
    backend
        .context()?
        .set_menubar_rings(rings, hidden_profiles)?;
    Ok(())
}

/// Sets the remote that keys projects and the default branch.
///
/// # Errors
///
/// A config that cannot be edited, or a `[git]` key that is not a table.
pub fn set_git_defaults(
    backend: &Backend,
    default_remote: &GitName,
    default_branch: Option<&GitName>,
) -> Result<(), CommandError> {
    backend
        .context()?
        .set_git_defaults(default_remote, default_branch)?;
    Ok(())
}

/// Raises the transcript retention of a Claude profile to ten years.
///
/// # Errors
///
/// The agent is not Claude, the profile is unknown, or `settings.json` cannot be edited.
pub fn keep_ten_years(
    backend: &Backend,
    agent: AgentId,
    profile: &ProfileId,
) -> Result<(), CommandError> {
    if agent != AgentId::CLAUDE {
        return Err(CommandError::invalid(format!(
            "{agent} has no retention setting to raise"
        )));
    }
    backend.context()?.set_claude_retention(profile)?;
    Ok(())
}

/// Removes the store, the backups, the logs and the background service. The config stays.
///
/// The service is unloaded first; a daemon outside it (`tokenmtr run` in a terminal) is asked
/// to shut down, and the erase waits for it to release the daemon lock.
///
/// # Errors
///
/// A file or launchd operation failed, or a daemon still holds the lock.
pub async fn erase_all_data(backend: &Backend) -> Result<(), CommandError> {
    if let Ok(mut client) = connect(backend).await
        && let Err(error) = client.shutdown().await
    {
        tracing::warn!(%error, "daemon refused to shut down before the erase");
    }
    let backend = backend.clone();
    super::blocking(move || {
        let service = LaunchdService::new(backend.paths(), SystemRunner);
        backend.erase(
            EraseScope {
                config: false,
                service: true,
            },
            &service,
        )?;
        Ok(())
    })
    .await
}

/// A rendered report as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GeneratedReport {
    /// Format, as the request spelled it.
    pub format: crate::request::FormatWire,
    /// File name to offer when saving.
    pub filename: String,
    /// The document (UTF-8 for all three formats).
    pub content: String,
}

/// Builds a client report.
///
/// # Errors
///
/// An empty or invalid selection.
pub fn generate_report(
    backend: &Backend,
    now: Timestamp,
    request: &ReportRequest,
) -> Result<GeneratedReport, CommandError> {
    let rendered = backend.context()?.generate_report(now, request)?;
    Ok(GeneratedReport {
        format: rendered.format.into(),
        filename: rendered.suggested_name,
        content: String::from_utf8(rendered.bytes).map_err(CommandError::internal)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fixture::{CONFIG, seeded};

    #[test]
    fn project_rule_escapes_the_glob_and_names_the_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let rule = project_rule_for(&fixture.backend, "/work/odd[1]/acme*", fixture.now)?;
        assert_eq!(rule.pattern, r"/work/odd\[1\]/acme\*");
        assert_eq!(rule.name.as_str(), "acme*");
        assert!(project_rule_for(&fixture.backend, "relative/dir", fixture.now).is_err());
        assert!(add_project_rule(&fixture.backend, "/work/x", "  ").is_err());
        Ok(())
    }

    #[test]
    fn reveal_creates_a_missing_config_file() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        std::fs::remove_file(fixture.backend.config_path()).ok();
        let path = reveal_path(&fixture.backend, &RevealTarget::ConfigFile)?;
        assert!(std::fs::read_to_string(path)?.starts_with('#'));
        Ok(())
    }

    #[test]
    fn reveal_resolves_known_places_only() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        std::fs::write(fixture.backend.config_path(), CONFIG)?;
        assert_eq!(
            reveal_path(&fixture.backend, &RevealTarget::ConfigFile)?,
            fixture.backend.config_path()
        );
        let ghost = RevealTarget::Transcript {
            agent: AgentId::CLAUDE,
            session: SessionId::try_from("ghost")?,
        };
        assert_eq!(
            reveal_path(&fixture.backend, &ghost).map_err(|e| e.kind),
            Err(ErrorKind::InvalidRequest)
        );
        Ok(())
    }

    #[test]
    fn set_menubar_rings_writes_the_config() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let work = ProfileId::try_from("work")?;
        set_menubar_rings(
            &fixture.backend,
            &[MenubarRing::PerModel],
            std::slice::from_ref(&work),
        )?;
        let text = std::fs::read_to_string(fixture.backend.config_path())?;
        assert!(text.contains("rings = [\"per_model\"]"), "{text}");
        let config = fixture.backend.context()?.config().menubar.clone();
        assert_eq!(config.rings, [MenubarRing::PerModel]);
        assert_eq!(config.hidden_profiles, [work]);
        assert!(
            set_menubar_rings(
                &fixture.backend,
                &[MenubarRing::Weekly, MenubarRing::Weekly],
                &[]
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn set_git_defaults_writes_the_config() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let upstream = GitName::try_from("upstream".to_owned())?;
        let main = GitName::try_from("main".to_owned())?;
        set_git_defaults(&fixture.backend, &upstream, Some(&main))?;
        let text = std::fs::read_to_string(fixture.backend.config_path())?;
        assert!(text.contains("default_remote = \"upstream\""), "{text}");
        assert!(text.contains("default_branch = \"main\""), "{text}");
        assert_eq!(
            fixture.backend.context()?.config().git.default_remote,
            upstream
        );
        set_git_defaults(&fixture.backend, &upstream, None)?;
        let text = std::fs::read_to_string(fixture.backend.config_path())?;
        assert!(!text.contains("default_branch"), "{text}");
        assert_eq!(fixture.backend.context()?.config().git.default_branch, None);
        Ok(())
    }

    #[test]
    fn add_project_rule_writes_the_config() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        add_project_rule(&fixture.backend, "/work/globex", "Globex site")?;
        let text = std::fs::read_to_string(fixture.backend.config_path())?;
        assert!(text.contains("[[projects]]"), "{text}");
        assert!(text.contains("/work/globex"), "{text}");
        assert!(text.contains("Globex site"), "{text}");
        assert_eq!(fixture.backend.context()?.config().projects.len(), 1);
        Ok(())
    }

    #[test]
    fn keep_ten_years_rejects_other_agents_and_unknown_profiles()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        assert_eq!(
            keep_ten_years(
                &fixture.backend,
                AgentId::PI,
                &ProfileId::try_from("default")?
            )
            .map_err(|e| e.kind),
            Err(ErrorKind::InvalidRequest)
        );
        assert!(
            keep_ten_years(
                &fixture.backend,
                AgentId::CLAUDE,
                &ProfileId::try_from("ghost")?
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn generate_report_renders_markdown() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        let request = ReportRequest {
            range: tokenmtr_app::ReportRange::Month(jiff::civil::date(2026, 5, 1)),
            scope: tokenmtr_report::Scope::All,
            format: tokenmtr_app::ReportFormat::Markdown,
            currency: None,
        };
        let report = generate_report(&fixture.backend, fixture.now, &request)?;
        assert!(
            std::path::Path::new(&report.filename)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md")),
            "{}",
            report.filename
        );
        assert!(report.content.contains("acme"), "{}", report.content);
        Ok(())
    }

    #[tokio::test]
    async fn scan_now_falls_back_to_the_process_without_a_daemon()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = seeded(CONFIG)?;
        scan_now(&fixture.backend).await?;
        export_now(&fixture.backend).await.map_or_else(
            |error| assert_eq!(error.kind, ErrorKind::DaemonOffline),
            |()| panic!("export needs a daemon"),
        );
        Ok(())
    }
}
