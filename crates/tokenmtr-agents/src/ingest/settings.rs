//! What the ingestor needs from the configuration, resolved once.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use tokenmtr_config::{
    Config, Env, GitName, ProfileLocation, ProjectRule, SourceTarget, TimeZoneSetting,
};
use tokenmtr_core::{AgentId, Override, SourceId};
use tokenmtr_platform::Paths;

use crate::project::ProjectResolver;
use crate::project::superset::Hints;
use crate::source::Profile;
use crate::{AgentKind, ClaudeSource, PiSource, claude, pi};

/// Environment variable naming Claude's config dir.
const CLAUDE_CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
/// Environment variable naming pi's session dir.
const PI_SESSION_DIR: &str = "PI_CODING_AGENT_SESSION_DIR";
/// Environment variable naming pi's agent dir.
const PI_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";

/// Everything a scan reads from configuration and the environment.
#[derive(Debug, Clone)]
pub struct IngestSettings {
    /// One collector per enabled agent, with its profiles.
    pub sources: Vec<AgentKind>,
    /// `[[projects]]` rules, in match order.
    pub rules: Vec<ProjectRule>,
    /// `[git] default_remote`: the remote whose URL keys a repository's project.
    pub default_remote: GitName,
    /// `[pricing.overrides]`.
    pub overrides: Vec<Override>,
    /// The display zone: stats-cache days sit at 12:00 in it.
    pub zone: TimeZone,
    /// `import.claude_stats_cache`.
    pub import_claude_stats_cache: bool,
    /// macOS protected folders: project resolution never reads cwds under them, and the
    /// doctor reports cwds and sinks under them.
    pub protected: Vec<Utf8PathBuf>,
    /// `privacy.redact_paths`: store keyed hashes instead of cwds and git branches.
    pub redact_paths: bool,
    /// The enabled `superset` sources of `[[sources]]` with their data directories, read for
    /// hints.
    pub superset: Vec<(SourceId, Utf8PathBuf)>,
    /// The age up to which an event without a logged branch takes its cwd's current HEAD:
    /// two scan intervals ([`super::projects::BranchStamp`]).
    pub live_branch_within: SignedDuration,
}

impl IngestSettings {
    /// The name a project rule for `cwd` suggests: what resolution would name it without
    /// rules, so its repository's (from git, or Superset's record of a deleted worktree), else
    /// its folder's. `None` for a blank cwd.
    #[must_use]
    pub fn suggested_project_name(&self, cwd: &Utf8Path, now: Timestamp) -> Option<String> {
        ProjectResolver::new(Vec::new(), self.protected.clone())
            .with_default_remote(self.default_remote.clone())
            .resolve(cwd, now, &mut self.hints())
            .map(|found| found.resolution.name)
    }

    /// The resolver of these settings: their rules, protected folders and default remote.
    #[must_use]
    pub fn resolver(&self) -> ProjectResolver {
        ProjectResolver::new(self.rules.clone(), self.protected.clone())
            .with_default_remote(self.default_remote.clone())
    }

    /// The hints of the enabled Superset sources, read on first use.
    pub(crate) fn hints(&self) -> Hints {
        Hints::superset(self.superset.clone())
    }

    /// Builds the settings for `config`. `env` is read for `CLAUDE_CONFIG_DIR`,
    /// `PI_CODING_AGENT_SESSION_DIR` and `PI_CODING_AGENT_DIR`, and pi's `settings.json` for
    /// `sessionDir`, which only the `default` profile honours.
    #[must_use]
    pub fn from_config(config: &Config, paths: &Paths, env: &dyn Env) -> Self {
        let home = paths.home();
        let dir = |name: &str| env.var(name).map(Utf8PathBuf::from);
        let claude_dir = dir(CLAUDE_CONFIG_DIR);
        let pi_sessions = dir(PI_SESSION_DIR);
        let pi_dir = dir(PI_AGENT_DIR);
        let pi_configured =
            pi::configured_session_dir(home, &pi::default_agent_dir(home, pi_dir.as_deref()));
        let sources = config
            .enabled_agents()
            .filter_map(|agent| {
                let profiles = |build: &dyn Fn(&tokenmtr_config::ProfileConfig) -> Profile| {
                    config
                        .agents
                        .get(&agent)
                        .map(|a| a.profiles.iter().map(build).collect::<Vec<_>>())
                        .unwrap_or_default()
                };
                if agent == AgentId::CLAUDE {
                    Some(AgentKind::Claude(ClaudeSource::new(profiles(&|p| {
                        claude_profile(p, home, claude_dir.as_deref())
                    }))))
                } else if agent == AgentId::PI {
                    Some(AgentKind::Pi(PiSource::new(profiles(&|p| {
                        pi_profile(
                            p,
                            home,
                            pi_sessions.as_deref(),
                            pi_dir.as_deref(),
                            pi_configured.as_deref(),
                        )
                    }))))
                } else {
                    None
                }
            })
            .collect();
        Self {
            sources,
            rules: config.projects.clone(),
            default_remote: config.git.default_remote.clone(),
            overrides: config.pricing.overrides.clone(),
            zone: match &config.display.timezone {
                TimeZoneSetting::System => TimeZone::system(),
                TimeZoneSetting::Named(zone) => zone.clone(),
            },
            import_claude_stats_cache: config.import.claude_stats_cache,
            protected: paths.protected(),
            redact_paths: config.privacy.redact_paths,
            superset: config
                .sources
                .iter()
                .filter(|source| source.enabled)
                .filter_map(|source| match &source.target {
                    SourceTarget::Superset(superset) => Some((
                        source.id.clone(),
                        superset
                            .dir
                            .clone()
                            .unwrap_or_else(|| home.join(".superset")),
                    )),
                    SourceTarget::Sqlite(_) => None,
                })
                .collect(),
            live_branch_within: config
                .daemon
                .scan_interval
                .checked_mul(2)
                .unwrap_or(config.daemon.scan_interval),
        }
    }
}

fn claude_profile(
    config: &tokenmtr_config::ProfileConfig,
    home: &Utf8Path,
    config_dir: Option<&Utf8Path>,
) -> Profile {
    match &config.location {
        ProfileLocation::Default => {
            let mut profile = claude::default_profile(home, config_dir);
            profile.id = config.id.clone();
            profile
        }
        ProfileLocation::Roots(roots) => Profile::new(config.id.clone(), roots.clone()),
        ProfileLocation::ConfigDir(dir) => claude::profile_for_config_dir(config.id.clone(), dir),
    }
}

fn pi_profile(
    config: &tokenmtr_config::ProfileConfig,
    home: &Utf8Path,
    session_dir: Option<&Utf8Path>,
    agent_dir: Option<&Utf8Path>,
    configured: Option<&Utf8Path>,
) -> Profile {
    match &config.location {
        ProfileLocation::Default => {
            let mut profile = pi::default_profile(home, session_dir, agent_dir, configured);
            profile.id = config.id.clone();
            profile
        }
        ProfileLocation::Roots(roots) => Profile::new(config.id.clone(), roots.clone()),
        ProfileLocation::ConfigDir(dir) => pi::profile_for_agent_dir(config.id.clone(), dir),
    }
}
