//! What the ingestor needs from the configuration, resolved once.

use camino::{Utf8Path, Utf8PathBuf};
use jiff::tz::TimeZone;
use tokenmtr_config::{Config, Env, ProfileLocation, ProjectRule, TimeZoneSetting};
use tokenmtr_core::{AgentId, Override};
use tokenmtr_platform::Paths;

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
    /// `[[projects]]` rules, in match order (D31).
    pub rules: Vec<ProjectRule>,
    /// `[pricing.overrides]` (D11).
    pub overrides: Vec<Override>,
    /// The display zone: stats-cache days sit at 12:00 in it (D38).
    pub zone: TimeZone,
    /// `import.claude_stats_cache` (D38).
    pub import_claude_stats_cache: bool,
    /// macOS protected folders the doctor reports cwds under (D50).
    pub protected: Vec<Utf8PathBuf>,
}

impl IngestSettings {
    /// Builds the settings for `config`. `env` is read for `CLAUDE_CONFIG_DIR`,
    /// `PI_CODING_AGENT_SESSION_DIR` and `PI_CODING_AGENT_DIR`, which only the `default`
    /// profile honours.
    #[must_use]
    pub fn from_config(config: &Config, paths: &Paths, env: &dyn Env) -> Self {
        let home = paths.home();
        let dir = |name: &str| env.var(name).map(Utf8PathBuf::from);
        let claude_dir = dir(CLAUDE_CONFIG_DIR);
        let pi_sessions = dir(PI_SESSION_DIR);
        let pi_dir = dir(PI_AGENT_DIR);
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
                        pi_profile(p, home, pi_sessions.as_deref(), pi_dir.as_deref())
                    }))))
                } else {
                    None
                }
            })
            .collect();
        Self {
            sources,
            rules: config.projects.clone(),
            overrides: config.pricing.overrides.clone(),
            zone: match &config.display.timezone {
                TimeZoneSetting::System => TimeZone::system(),
                TimeZoneSetting::Named(zone) => zone.clone(),
            },
            import_claude_stats_cache: config.import.claude_stats_cache,
            protected: paths.protected().into(),
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
) -> Profile {
    match &config.location {
        ProfileLocation::Default => {
            let mut profile = pi::default_profile(home, session_dir, agent_dir);
            profile.id = config.id.clone();
            profile
        }
        ProfileLocation::Roots(roots) => Profile::new(config.id.clone(), roots.clone()),
        ProfileLocation::ConfigDir(dir) => pi::profile_for_agent_dir(config.id.clone(), dir),
    }
}
