//! pi collector: profiles, session discovery and parsing.
//!
//! A session file is JSONL: a header line, then one entry per line. Entries form a tree through
//! `id`/`parentId`, and a fork copies the entries of its parent under the same ids into a new
//! file, so the dedup key (see [`parse_line`]) is what counts each response once.

pub mod header;
mod line;

use camino::{Utf8Path, Utf8PathBuf};
use tokenmtr_core::{AgentId, Capabilities, ProfileId};

use crate::AgentSource;
use crate::batch::ParseBatch;
use crate::drift::{DriftStats, LineClass, ParseIssue, ParseOutcome};
use crate::error::AgentError;
use crate::jsonl::{self, NeedleSet};
use crate::skeleton::KeySkeleton;
use crate::source::{ByteOffset, Profile, SourceFile};

pub use header::{NEWEST_KNOWN_VERSION, SessionHeader};
pub use line::{LineContext, UNATTRIBUTED_MODEL, parse_line};

/// Lines without one of these are skipped unparsed. Every line that yields output holds one:
/// usage blocks hold `usage`, tool calls `toolCall`, tool results `toolResult`, compaction
/// signals `compaction`. The header line is read separately, so it needs none.
const LINE_NEEDLES: &[&[u8]] = &[b"usage", b"toolCall", b"toolResult", b"compaction"];

/// The pi collector.
#[derive(Debug, Clone)]
pub struct PiSource {
    profiles: Vec<Profile>,
    needles: NeedleSet,
}

impl PiSource {
    /// A collector over `profiles`. Build them with [`default_profile`] and
    /// [`profile_for_agent_dir`].
    #[must_use]
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self {
            profiles,
            needles: NeedleSet::new(LINE_NEEDLES),
        }
    }

    /// Turns the line pre-filter off: every line is parsed. The output must not change.
    #[must_use]
    pub fn without_prefilter(mut self) -> Self {
        self.needles = NeedleSet::all();
        self
    }
}

/// The `default` profile. Its root is `$PI_CODING_AGENT_SESSION_DIR` when set. Otherwise it is
/// `<agent dir>/sessions`, the agent dir being `$PI_CODING_AGENT_DIR` when set, else
/// `~/.pi/agent`; plus `configured`, the `sessionDir` of pi's settings
/// ([`configured_session_dir`]), where pi writes new sessions while older ones stay put. The
/// caller reads the environment and the settings; this stays pure.
#[must_use]
pub fn default_profile(
    home: &Utf8Path,
    session_dir: Option<&Utf8Path>,
    agent_dir: Option<&Utf8Path>,
    configured: Option<&Utf8Path>,
) -> Profile {
    if let Some(sessions) = session_dir {
        return Profile::new(ProfileId::default_profile(), vec![sessions.to_owned()]);
    }
    let mut roots: Vec<Utf8PathBuf> = configured.map(Utf8Path::to_owned).into_iter().collect();
    let sessions = default_agent_dir(home, agent_dir).join("sessions");
    if !roots.contains(&sessions) {
        roots.push(sessions);
    }
    Profile::new(ProfileId::default_profile(), roots)
}

/// pi's agent dir: `agent_dir` (`$PI_CODING_AGENT_DIR`) when set, else `~/.pi/agent`.
#[must_use]
pub fn default_agent_dir(home: &Utf8Path, agent_dir: Option<&Utf8Path>) -> Utf8PathBuf {
    agent_dir.map_or_else(|| home.join(".pi").join("agent"), Utf8Path::to_owned)
}

/// The `sessionDir` of pi's `<agent_dir>/settings.json`, with a leading `~` expanded. `None`
/// when the file or key is absent, unreadable, or the path is relative (pi would resolve it
/// against its own working directory, which is unknown here).
#[must_use]
pub fn configured_session_dir(home: &Utf8Path, agent_dir: &Utf8Path) -> Option<Utf8PathBuf> {
    #[derive(serde::Deserialize)]
    struct Settings {
        #[serde(rename = "sessionDir")]
        session_dir: Option<String>,
    }
    let text = std::fs::read_to_string(agent_dir.join("settings.json")).ok()?;
    let dir = serde_json::from_str::<Settings>(&text).ok()?.session_dir?;
    let dir = match dir.trim() {
        "~" => home.to_owned(),
        other => match other.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => Utf8PathBuf::from(other),
        },
    };
    dir.is_absolute().then_some(dir)
}

/// A profile whose pi agent dir is `agent_dir` (used through `PI_CODING_AGENT_DIR`): the one root
/// is `<agent_dir>/sessions`.
#[must_use]
pub fn profile_for_agent_dir(id: ProfileId, agent_dir: &Utf8Path) -> Profile {
    Profile::new(id, vec![agent_dir.join("sessions")])
}

impl AgentSource for PiSource {
    fn id(&self) -> AgentId {
        AgentId::PI
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            quota: false,
            reported_cost: true,
            subscription: true,
        }
    }

    fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    fn owns(&self, path: &Utf8Path) -> bool {
        path.extension() == Some("jsonl") && self.profiles.iter().any(|p| p.contains(path))
    }

    /// Parses the complete lines of `file` from `start`. The header is read from the first line of
    /// the file whatever `start` is, so a resumed parse yields what a whole-file parse would.
    ///
    /// A header newer than [`NEWEST_KNOWN_VERSION`] adds a [`ParseIssue::UnknownVersion`] on every
    /// call and is recorded in [`DriftStats::unknown_versions`]; the entries are still
    /// parsed, and drift detection tells whether they still map.
    fn parse(
        &self,
        file: &SourceFile,
        start: ByteOffset,
        out: &mut ParseBatch,
    ) -> Result<ParseOutcome, AgentError> {
        let header = header::read(&file.path)?;
        let ctx = LineContext::new(file, header.as_ref())?;
        let mut stats = DriftStats::default();
        let mut issues = Vec::new();
        if let Some(version) = header.as_ref().and_then(SessionHeader::unknown_version) {
            stats.unknown_versions.insert(version.to_string());
            issues.push(ParseIssue::UnknownVersion {
                version: version.to_string(),
            });
        }
        let mut sample = None;
        let scan = jsonl::read_lines(&file.path, start, &self.needles, |bytes| {
            let class = parse_line(&ctx, bytes, out);
            stats.record(class);
            if class == LineClass::Unmapped && sample.is_none() {
                sample = Some(KeySkeleton::from_json(bytes));
            }
        })?;
        stats.record_scan(start, &scan);
        issues.extend(stats.unmapped_issue(sample));
        Ok(ParseOutcome {
            new_offset: scan.new_offset,
            last_line_hash: scan.last_line_hash,
            issues,
            drift: stats,
        })
    }

    fn line_needles(&self) -> &'static [&'static [u8]] {
        LINE_NEEDLES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(profile: &Profile) -> Vec<&str> {
        profile.roots.iter().map(|root| root.as_str()).collect()
    }

    #[test]
    fn default_profile_root_follows_the_environment_chain() {
        let home = Utf8Path::new("/home/u");
        let sessions = Utf8Path::new("/s");
        let agent = Utf8Path::new("/a");
        let configured = Utf8Path::new("/c");
        assert_eq!(
            roots(&default_profile(
                home,
                Some(sessions),
                Some(agent),
                Some(configured)
            )),
            ["/s"]
        );
        assert_eq!(
            roots(&default_profile(home, None, Some(agent), None)),
            ["/a/sessions"]
        );
        assert_eq!(
            roots(&default_profile(home, None, None, None)),
            ["/home/u/.pi/agent/sessions"]
        );
        assert_eq!(
            roots(&default_profile(home, None, None, Some(configured))),
            ["/c", "/home/u/.pi/agent/sessions"]
        );
    }

    #[test]
    fn configured_session_dir_reads_pi_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = Utf8Path::from_path(tmp.path()).unwrap();
        let home = Utf8Path::new("/home/u");
        assert_eq!(configured_session_dir(home, agent), None);
        let write = |json: &str| std::fs::write(agent.join("settings.json"), json).unwrap();
        write(r#"{"sessionDir": "~/pi-sessions", "theme": "dark"}"#);
        assert_eq!(
            configured_session_dir(home, agent),
            Some(Utf8PathBuf::from("/home/u/pi-sessions"))
        );
        write(r#"{"sessionDir": "relative/dir"}"#);
        assert_eq!(configured_session_dir(home, agent), None);
        write(r#"{"theme": "dark"}"#);
        assert_eq!(configured_session_dir(home, agent), None);
    }

    #[test]
    fn owns_only_jsonl_under_a_root() {
        let source = PiSource::new(vec![profile_for_agent_dir(
            ProfileId::default_profile(),
            Utf8Path::new("/a"),
        )]);
        assert!(source.owns(Utf8Path::new("/a/sessions/--w--/t_s.jsonl")));
        assert!(!source.owns(Utf8Path::new("/a/sessions/--w--/t_s.json")));
        assert!(!source.owns(Utf8Path::new("/a/settings.jsonl")));
    }
}
