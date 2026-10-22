//! Identifier newtypes. Each is validated once at construction (parse, don't validate).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

/// Why an identifier was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The value is empty or whitespace only.
    #[error("{kind} must not be blank")]
    Blank {
        /// Which identifier type rejected the value.
        kind: &'static str,
    },
    /// The value is not a slug (1–64 chars of `[A-Za-z0-9._-]`).
    #[error("{kind} {value:?} must be 1-64 characters of [A-Za-z0-9._-]")]
    NotSlug {
        /// Which identifier type rejected the value.
        kind: &'static str,
        /// The rejected value.
        value: String,
    },
    /// The agent id is not one of the known agents.
    #[error("unknown agent id {0:?}")]
    UnknownAgent(String),
    /// The text is not 32 lowercase hex characters.
    #[error("invalid event id {0:?}")]
    BadEventId(String),
    /// The machine id UUID is nil.
    #[error("machine id must not be the nil UUID")]
    NilMachineId,
    /// The text is not a UUID.
    #[error("invalid machine id {0:?}")]
    BadMachineId(String),
    /// The skill name is untrimmed, longer than 128 bytes or holds a control character.
    /// The value is left out: it comes from tool arguments, which stay out of logs.
    #[error("invalid skill name")]
    BadSkillName,
}

fn non_blank(kind: &'static str, value: &str) -> Result<(), IdError> {
    if value.trim().is_empty() {
        Err(IdError::Blank { kind })
    } else {
        Ok(())
    }
}

fn slug(kind: &'static str, value: &str) -> Result<(), IdError> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(IdError::NotSlug {
            kind,
            value: value.to_owned(),
        })
    }
}

fn skill_name(kind: &'static str, value: &str) -> Result<(), IdError> {
    non_blank(kind, value)?;
    let ok = value.len() <= 128 && value.trim() == value && !value.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(IdError::BadSkillName)
    }
}

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident, $check:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl $name {
            /// The identifier as text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(value: String) -> Result<Self, IdError> {
                $check(stringify!($name), &value)?;
                Ok(Self(value))
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;
            fn try_from(value: &str) -> Result<Self, IdError> {
                Self::try_from(value.to_owned())
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(value: &str) -> Result<Self, IdError> {
                Self::try_from(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }
    };
}

string_id!(
    /// A model id exactly as reported by the agent (non-blank).
    ModelId, non_blank
);
string_id!(
    /// The company serving a model, e.g. `anthropic` (non-blank).
    ProviderId, non_blank
);
string_id!(
    /// An agent session id (non-blank).
    SessionId, non_blank
);
string_id!(
    /// A per-agent string identifying one usage event across lines and files (non-blank).
    DedupKey, non_blank
);
string_id!(
    /// A project key: normalized git remote (`github.com/owner/repo`) or a path (non-blank).
    ProjectKey, non_blank
);
string_id!(
    /// A profile id such as `default` or `work` (slug).
    ProfileId, slug
);
string_id!(
    /// A sink id from config (slug).
    SinkId, slug
);
string_id!(
    /// A source id from config (slug).
    SourceId, slug
);
string_id!(
    /// A budget id from config (slug).
    BudgetId, slug
);
string_id!(
    /// A subscription id from config (slug).
    SubscriptionId, slug
);
string_id!(
    /// The name of an agent skill a tool call loaded, e.g. `git-commit` or `plugin:skill`:
    /// trimmed, non-blank, at most 128 bytes, no control characters. The only value the
    /// store keeps from tool arguments.
    SkillName, skill_name
);

impl SourceId {
    /// The id of the Superset source a config without one gets.
    #[must_use]
    pub fn implicit_superset() -> Self {
        Self("superset".to_owned())
    }
}

impl ProfileId {
    /// The implicit profile every agent has.
    #[must_use]
    pub fn default_profile() -> Self {
        Self("default".to_owned())
    }
}

/// A coding agent: a closed set, so the id is always one of the known `'static` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentId(&'static str);

impl AgentId {
    /// Claude Code.
    pub const CLAUDE: Self = Self("claude");
    /// pi.
    pub const PI: Self = Self("pi");
    /// Every known agent. A new agent adds one entry here.
    pub const KNOWN: &'static [Self] = &[Self::CLAUDE, Self::PI];

    /// The agent id as text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl FromStr for AgentId {
    type Err = IdError;
    fn from_str(value: &str) -> Result<Self, IdError> {
        Self::KNOWN
            .iter()
            .copied()
            .find(|a| a.0 == value)
            .ok_or_else(|| IdError::UnknownAgent(value.to_owned()))
    }
}

impl TryFrom<&str> for AgentId {
    type Error = IdError;
    fn try_from(value: &str) -> Result<Self, IdError> {
        value.parse()
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for AgentId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for AgentId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Random per-installation identifier (UUID, never nil) created on first run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "Uuid")]
pub struct MachineId(Uuid);

impl MachineId {
    /// The underlying UUID.
    #[must_use]
    pub const fn uuid(self) -> Uuid {
        self.0
    }
}

impl TryFrom<Uuid> for MachineId {
    type Error = IdError;
    fn try_from(value: Uuid) -> Result<Self, IdError> {
        if value.is_nil() {
            Err(IdError::NilMachineId)
        } else {
            Ok(Self(value))
        }
    }
}

impl FromStr for MachineId {
    type Err = IdError;
    fn from_str(value: &str) -> Result<Self, IdError> {
        let uuid = Uuid::parse_str(value).map_err(|_| IdError::BadMachineId(value.to_owned()))?;
        Self::try_from(uuid)
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Row id of a source file in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceFileId(i64);

impl SourceFileId {
    /// Wraps a store row id.
    #[must_use]
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    /// The store row id.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Globally unique usage-event id: `blake3(machine_id ‖ agent ‖ 0x00 ‖ dedup_key)` truncated to
/// 16 bytes. The NUL separator keeps `("pi", "xfoo")` and `("pix", "foo")` apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId([u8; 16]);

impl EventId {
    /// Derives the id of one event.
    #[must_use]
    pub fn new(machine: &MachineId, agent: AgentId, dedup_key: &DedupKey) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(machine.0.as_bytes());
        hasher.update(agent.as_str().as_bytes());
        hasher.update(&[0]);
        hasher.update(dedup_key.as_str().as_bytes());
        let hash = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        Self(bytes)
    }

    /// Rebuilds an id read back from the store.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The raw 16 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl FromStr for EventId {
    type Err = IdError;
    fn from_str(value: &str) -> Result<Self, IdError> {
        let bad = || IdError::BadEventId(value.to_owned());
        if value.len() != 32 || !value.is_ascii() {
            return Err(bad());
        }
        let mut bytes = [0_u8; 16];
        for (byte, pair) in bytes.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
            let text = std::str::from_utf8(pair).map_err(|_| bad())?;
            *byte = u8::from_str_radix(text, 16).map_err(|_| bad())?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for EventId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn machine(n: u128) -> Result<MachineId, IdError> {
        MachineId::try_from(Uuid::from_u128(n))
    }

    #[test]
    fn blank_ids_are_rejected() {
        assert_eq!(
            ModelId::try_from("  "),
            Err(IdError::Blank { kind: "ModelId" })
        );
        assert!(SessionId::try_from("").is_err());
    }

    #[test]
    fn slugs_reject_spaces_and_long_values() {
        assert!(ProfileId::try_from("work-2").is_ok());
        assert!(ProfileId::try_from("has space").is_err());
        assert!(SinkId::try_from("a".repeat(65)).is_err());
        assert!(SourceId::try_from("other-machines").is_ok());
        assert!(SourceId::try_from("has space").is_err());
        assert!(BudgetId::try_from("").is_err());
    }

    #[test]
    fn skill_names_are_trimmed_short_and_printable() {
        assert!(SkillName::try_from("git-commit").is_ok());
        assert!(SkillName::try_from("plugin:skill name").is_ok());
        assert!(SkillName::try_from("a".repeat(128)).is_ok());
        assert_eq!(
            SkillName::try_from("a".repeat(129)),
            Err(IdError::BadSkillName)
        );
        assert_eq!(
            SkillName::try_from(" git-commit"),
            Err(IdError::BadSkillName)
        );
        assert_eq!(
            SkillName::try_from("git\ncommit"),
            Err(IdError::BadSkillName)
        );
        assert_eq!(
            SkillName::try_from(" \t"),
            Err(IdError::Blank { kind: "SkillName" })
        );
    }

    #[test]
    fn default_profile_is_valid() {
        assert_eq!(ProfileId::default_profile().as_str(), "default");
    }

    #[test]
    fn agent_ids_parse_known_only() -> TestResult {
        assert_eq!("claude".parse::<AgentId>()?, AgentId::CLAUDE);
        assert!("codex".parse::<AgentId>().is_err());
        assert_eq!(serde_json::to_string(&AgentId::PI)?, "\"pi\"");
        assert_eq!(serde_json::from_str::<AgentId>("\"pi\"")?, AgentId::PI);
        Ok(())
    }

    #[test]
    fn serde_validates_on_deserialize() {
        assert!(serde_json::from_str::<ModelId>("\"\"").is_err());
        assert!(
            serde_json::from_str::<MachineId>("\"00000000-0000-0000-0000-000000000000\"").is_err()
        );
    }

    #[test]
    fn nil_machine_id_is_rejected() {
        assert_eq!(machine(0), Err(IdError::NilMachineId));
    }

    #[test]
    fn event_id_separates_agent_from_key() -> TestResult {
        let m = machine(1)?;
        let a = EventId::new(&m, AgentId::PI, &DedupKey::try_from("xfoo")?);
        let b = EventId::new(&m, AgentId::CLAUDE, &DedupKey::try_from("xfoo")?);
        assert_ne!(a, b);
        Ok(())
    }

    #[test]
    fn event_id_hex_round_trips() -> TestResult {
        let id = EventId::new(
            &machine(7)?,
            AgentId::CLAUDE,
            &DedupKey::try_from("msg|req")?,
        );
        assert_eq!(id.to_string().parse::<EventId>()?, id);
        assert_eq!(
            serde_json::from_str::<EventId>(&serde_json::to_string(&id)?)?,
            id
        );
        assert!("zz".parse::<EventId>().is_err());
        Ok(())
    }

    proptest! {
        #[test]
        fn event_id_is_deterministic_and_machine_scoped(key in "[a-z0-9|]{1,24}", m in 1_u128..u128::MAX) {
            let dedup = DedupKey::try_from(key.as_str()).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let m1 = machine(m).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let m2 = machine(m ^ 1).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(EventId::new(&m1, AgentId::CLAUDE, &dedup), EventId::new(&m1, AgentId::CLAUDE, &dedup));
            prop_assume!(m ^ 1 != 0);
            prop_assert_ne!(EventId::new(&m1, AgentId::CLAUDE, &dedup), EventId::new(&m2, AgentId::CLAUDE, &dedup));
        }
    }
}
