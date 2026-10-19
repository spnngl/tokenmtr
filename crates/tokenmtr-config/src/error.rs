//! Errors, each carrying where in the file it happened.

use std::fmt;
use std::io;

use camino::Utf8PathBuf;
use tokenmtr_core::billing::{BillingError, PlanCatalogError};
use tokenmtr_core::budget::BudgetError;
use tokenmtr_core::{
    AgentId, BillingPeriod, Currency, MoneyError, Plan, PricingError, ProfileId, SinkId,
    SubscriptionId,
};

/// One step of a [`ConfigPath`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// A table key.
    Key(String),
    /// An array index.
    Index(usize),
}

/// The location of a value inside the config, independent of the file format, e.g.
/// `sinks[1].id`. Codecs turn it into a source span.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigPath(Vec<Segment>);

impl ConfigPath {
    /// The document root.
    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    /// This path extended with a table key.
    #[must_use]
    pub fn key(&self, key: &str) -> Self {
        self.push(Segment::Key(key.to_owned()))
    }

    /// This path extended with an array index.
    #[must_use]
    pub fn index(&self, index: usize) -> Self {
        self.push(Segment::Index(index))
    }

    /// The steps from the root.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.0
    }

    fn push(&self, segment: Segment) -> Self {
        let mut segments = self.0.clone();
        segments.push(segment);
        Self(segments)
    }
}

impl fmt::Display for ConfigPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            match segment {
                Segment::Key(key) if i == 0 => f.write_str(key)?,
                Segment::Key(key) => write!(f, ".{key}")?,
                Segment::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

/// A 1-based line and column in the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// Line, starting at 1.
    pub line: usize,
    /// Column in characters, starting at 1.
    pub column: usize,
}

impl LineCol {
    /// Converts a byte offset into `source`; offsets past the end clamp to the end.
    #[must_use]
    pub fn at(source: &str, offset: usize) -> Self {
        let mut end = offset.min(source.len());
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        let before = &source[..end];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        Self {
            line,
            column: before[line_start..].chars().count() + 1,
        }
    }
}

/// Where an error happened: the file (when loaded from one) and the position (when the codec
/// could point at one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The config file, `None` for text parsed from memory.
    pub file: Option<Utf8PathBuf>,
    /// The position inside the file.
    pub position: Option<LineCol>,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.position) {
            (Some(file), Some(p)) => write!(f, "{file}:{}:{}", p.line, p.column),
            (Some(file), None) => write!(f, "{file}"),
            (None, Some(p)) => write!(f, "<config>:{}:{}", p.line, p.column),
            (None, None) => f.write_str("<config>"),
        }
    }
}

/// A rule of the config that the file breaks, one variant per rule.
#[derive(Debug, thiserror::Error)]
pub enum Violation {
    /// `schema_version` is not [`CONFIG_SCHEMA_VERSION`](crate::CONFIG_SCHEMA_VERSION).
    #[error("schema_version {found} is not supported (this build reads {supported})")]
    UnsupportedSchemaVersion {
        /// Version in the file.
        found: u32,
        /// Version this build reads.
        supported: u32,
    },
    /// A path starts with `~` but `$HOME` is unset or relative.
    #[error("{path:?} starts with `~` but $HOME is not an absolute path")]
    HomeUnset {
        /// The path as written.
        path: String,
    },
    /// A path is relative; the daemon has no stable working directory.
    #[error("{path:?} must be an absolute path (or start with `~`)")]
    RelativePath {
        /// The path as written.
        path: String,
    },
    /// A list that must name at least one entry is empty.
    #[error("list must not be empty; remove the key to use the default")]
    EmptyList,
    /// A list names the same entry twice.
    #[error("{0:?} is listed twice")]
    DuplicateEntry(String),
    /// Two entries of a table array share an id.
    #[error("duplicate {kind} id {id:?}")]
    DuplicateId {
        /// What kind of entry (`subscription`, `budget`, `profile`).
        kind: &'static str,
        /// The repeated id.
        id: String,
    },
    /// Two sinks share an id.
    #[error("duplicate sink id {0}")]
    DuplicateSinkId(SinkId),
    /// The sink would write the same files as an earlier one: same kind, same `dir` or `path`
    /// (unset means the exports directory) and a dataset in common.
    #[error("writes the same files as sink {0}")]
    SinkTargetClash(SinkId),
    /// A sink sets a key its kind does not use.
    #[error("sink kind `{kind}` does not accept `{field}`")]
    FieldNotAllowed {
        /// The sink kind.
        kind: &'static str,
        /// The offending key.
        field: &'static str,
    },
    /// A metrics output reuses an id of a sink or of another output.
    #[error("id {0} is already used by a sink or another metrics output")]
    DuplicateOutputId(SinkId),
    /// `exclude_paths` cannot match: `privacy.redact_paths` hashes every working directory at
    /// ingest.
    #[error(
        "exclude_paths cannot match hashed paths; turn off privacy.redact_paths or use exclude_projects"
    )]
    ExcludePathsWithRedactedPaths,
    /// `redact` names a column that is not a metric label.
    #[error("`{0}` is not a metric label; metrics never carry the working directory")]
    NotAMetricLabel(&'static str),
    /// `listen` is not a socket address.
    #[error("invalid listen address {value:?}: {reason}")]
    InvalidListen {
        /// The value as written.
        value: String,
        /// Why it was rejected.
        reason: String,
    },
    /// Two exporters listen on the same address.
    #[error("listens on the same address as exporter {0}")]
    ListenClash(SinkId),
    /// `url` is not an absolute http(s) URL with a host.
    #[error("invalid url: {reason}")]
    ///
    /// The value is not echoed: it may hold a credential in its user info or query.
    InvalidUrl {
        /// Why it was rejected.
        reason: String,
    },
    /// `url` is plain http to a host that is not this machine.
    #[error(
        "url must use https unless it points at this machine (localhost or a loopback address)"
    )]
    InsecureUrl,
    /// `url` carries credentials; they belong under `auth`.
    #[error("url must not contain credentials; use `auth`")]
    UrlCredentials,
    /// An `auth` table gives a secret in zero or several ways.
    #[error("give exactly one of {options}")]
    SecretSourceCount {
        /// The accepted keys, as written in config.
        options: &'static str,
    },
    /// An `auth` table sets a key its `kind` does not use.
    #[error("auth kind `{kind}` does not accept `{field}`")]
    AuthFieldNotAllowed {
        /// The auth kind.
        kind: &'static str,
        /// The offending key.
        field: &'static str,
    },
    /// `auth.kind = "basic"` has no `username`.
    #[error("auth kind `basic` needs `username`")]
    UsernameRequired,
    /// A basic-auth `username` contains `:`, which HTTP basic auth cannot carry.
    #[error("username must not contain `:`")]
    UsernameColon,
    /// A text value that must carry something is empty or blank.
    #[error("value must not be blank")]
    BlankValue,
    /// A value contains a control character or, for a header, a newline.
    #[error("value must be printable text on one line")]
    NotPrintable,
    /// An `external_labels` key is not a Prometheus label name.
    #[error("{0:?} is not a valid label name: use [a-zA-Z_][a-zA-Z0-9_]* and no `__` prefix")]
    InvalidLabelName(String),
    /// An `external_labels` key is a label the metrics already carry.
    #[error("label {0:?} is already set by the metrics and cannot be an external label")]
    ReservedLabel(String),
    /// A list names a disabled agent.
    #[error("agent `{0}` is disabled; enable it under [agents.{0}] or remove it here")]
    AgentDisabled(AgentId),
    /// A list names a profile no agent in scope declares.
    #[error("profile `{0}` is not declared under any agent in scope")]
    UnknownProfile(ProfileId),
    /// Two subscriptions would both cover one event.
    #[error("subscriptions {first} and {second} overlap on provider, agent, profile and dates")]
    OverlappingSubscriptions {
        /// The earlier entry.
        first: SubscriptionId,
        /// The later entry, where the error points.
        second: SubscriptionId,
    },
    /// A subscription omits `fee` and the plan catalog has no list price for it.
    #[error(
        "subscription {subscription} needs an explicit `fee`: no {period:?} {currency:?} list price for plan {plan:?}"
    )]
    FeeRequired {
        /// The subscription.
        subscription: SubscriptionId,
        /// Its plan.
        plan: Plan,
        /// Its billing period.
        period: BillingPeriod,
        /// Its fee currency.
        currency: Currency,
    },
    /// Two profiles of one agent read from the same or nested paths.
    #[error("profile `{profile}` of agent `{agent}` shares {path} with profile `{other}`")]
    SharedRoot {
        /// The agent.
        agent: AgentId,
        /// The later profile, where the error points.
        profile: ProfileId,
        /// The earlier profile.
        other: ProfileId,
        /// The contested path.
        path: Utf8PathBuf,
    },
    /// A profile sets both `roots` and `config_dir`.
    #[error("profile `{0}` sets both `roots` and `config_dir`; use one")]
    RootsAndConfigDir(ProfileId),
    /// A `[[projects]]` match is not a valid glob.
    #[error("invalid glob {pattern:?}: {reason}")]
    InvalidGlob {
        /// The pattern after `~` expansion.
        pattern: String,
        /// Why the glob crate rejected it.
        reason: String,
    },
    /// `display.timezone` is neither `system` nor a known IANA name.
    #[error("unknown time zone {name:?}: {reason}")]
    InvalidTimeZone {
        /// The name as written.
        name: String,
        /// Why the lookup failed.
        reason: String,
    },
    /// An interval is shorter than the minimum.
    #[error("interval must be at least {min}")]
    IntervalTooShort {
        /// The minimum, as written in config.
        min: &'static str,
    },
    /// `backup.keep` is zero.
    #[error("backup.keep must be at least 1")]
    ZeroKeep,
    /// A money amount is out of range.
    #[error(transparent)]
    Money(#[from] MoneyError),
    /// A subscription breaks a billing rule.
    #[error(transparent)]
    Subscription(#[from] BillingError),
    /// A budget breaks a budget rule.
    #[error(transparent)]
    Budget(#[from] BudgetError),
    /// A pricing override has an invalid rate.
    #[error(transparent)]
    Pricing(#[from] PricingError),
    /// The embedded plan catalog does not parse (a build bug).
    #[error(transparent)]
    PlanCatalog(#[from] Box<PlanCatalogError>),
}

/// A [`Violation`] and the config path it points at.
#[derive(Debug)]
pub struct Rejection {
    /// The offending value.
    pub path: ConfigPath,
    /// The rule it breaks.
    pub violation: Violation,
}

/// Why a config could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: Utf8PathBuf,
        /// The I/O error.
        #[source]
        source: io::Error,
    },
    /// The file extension names no known format.
    #[error("{path}: unsupported config format (extension {extension:?}); supported: toml")]
    UnsupportedFormat {
        /// The file.
        path: Utf8PathBuf,
        /// The extension, when there is one.
        extension: Option<String>,
    },
    /// The text does not match the schema: invalid syntax, an unknown key, a wrong type, a bad
    /// duration, an unknown sink kind, and so on. `message` is the codec's description.
    #[error("{at}: {message}")]
    Syntax {
        /// Where it happened.
        at: Location,
        /// What is wrong.
        message: String,
    },
    /// The text parses but breaks a rule of the config.
    #[error("{at}: {path}: {violation}")]
    Invalid {
        /// Where it happened.
        at: Location,
        /// The offending value.
        path: ConfigPath,
        /// The rule it breaks.
        #[source]
        violation: Box<Violation>,
    },
    /// An edit could not be applied to the document.
    #[error("cannot edit config: {0}")]
    Edit(#[from] crate::format::EditError),
}
