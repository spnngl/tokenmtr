//! Typed filters: parsed once at the boundary, bound as SQL parameters, never spliced.

use std::str::FromStr;

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};
use tokenmtr_core::{AgentId, IdError, ModelId, ProfileId, ProjectKey, SubscriptionId};

use crate::context::QueryContext;
use crate::error::FilterError;
use crate::range::{DateRange, RangeSpec};

/// A client label from `[[projects]]` rules (non-blank).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ClientName(String);

impl ClientName {
    /// The label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ClientName {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, IdError> {
        if value.trim().is_empty() {
            Err(IdError::Blank { kind: "client" })
        } else {
            Ok(Self(value))
        }
    }
}

/// One profile restriction. Profile names are unique only within an agent (D30), so a profile
/// can be pinned to its agent: `claude/work`. A bare `work` matches that name under any agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileSelector {
    /// The agent the profile belongs to; `None` matches any agent.
    pub agent: Option<AgentId>,
    /// The profile name.
    pub profile: ProfileId,
}

impl ProfileSelector {
    /// Whether the selector picks `profile` of `agent`.
    #[must_use]
    pub fn matches(&self, agent: AgentId, profile: &ProfileId) -> bool {
        self.agent.is_none_or(|a| a == agent) && self.profile == *profile
    }
}

impl FromStr for ProfileSelector {
    type Err = IdError;

    /// Parses `agent/profile` or a bare `profile`.
    fn from_str(value: &str) -> Result<Self, IdError> {
        Ok(match value.split_once('/') {
            Some((agent, profile)) => Self {
                agent: Some(agent.parse()?),
                profile: profile.parse()?,
            },
            None => Self {
                agent: None,
                profile: value.parse()?,
            },
        })
    }
}

/// Restricts events by how they are billed (derived at query time, D28).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingFilter {
    /// Every event.
    #[default]
    Any,
    /// Only pay-as-you-go events.
    Api,
    /// Events covered by any subscription (`None`) or by one subscription.
    Subscription(Option<SubscriptionId>),
}

/// What to aggregate. Empty lists mean "no restriction"; non-empty lists are OR within a
/// dimension and AND across dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filters {
    range: DateRange,
    agents: Vec<AgentId>,
    profiles: Vec<ProfileSelector>,
    models: Vec<ModelId>,
    projects: Vec<ProjectKey>,
    clients: Vec<ClientName>,
    billing: BillingFilter,
}

impl Filters {
    /// Everything inside `range`.
    #[must_use]
    pub const fn new(range: DateRange) -> Self {
        Self {
            range,
            agents: Vec::new(),
            profiles: Vec::new(),
            models: Vec::new(),
            projects: Vec::new(),
            clients: Vec::new(),
            billing: BillingFilter::Any,
        }
    }

    /// Restricts to these agents.
    #[must_use]
    pub fn agents(mut self, agents: impl IntoIterator<Item = AgentId>) -> Self {
        self.agents = agents.into_iter().collect();
        self
    }

    /// Restricts to these profile names, under any agent.
    #[must_use]
    pub fn profiles(mut self, profiles: impl IntoIterator<Item = ProfileId>) -> Self {
        self.profiles = profiles
            .into_iter()
            .map(|profile| ProfileSelector {
                agent: None,
                profile,
            })
            .collect();
        self
    }

    /// Restricts to these profiles, each pinned to its agent or not.
    #[must_use]
    pub fn profile_selectors(
        mut self,
        profiles: impl IntoIterator<Item = ProfileSelector>,
    ) -> Self {
        self.profiles = profiles.into_iter().collect();
        self
    }

    /// Restricts to these models.
    #[must_use]
    pub fn models(mut self, models: impl IntoIterator<Item = ModelId>) -> Self {
        self.models = models.into_iter().collect();
        self
    }

    /// Restricts to these projects.
    #[must_use]
    pub fn projects(mut self, projects: impl IntoIterator<Item = ProjectKey>) -> Self {
        self.projects = projects.into_iter().collect();
        self
    }

    /// Restricts to these clients.
    #[must_use]
    pub fn clients(mut self, clients: impl IntoIterator<Item = ClientName>) -> Self {
        self.clients = clients.into_iter().collect();
        self
    }

    /// Restricts by billing mode.
    #[must_use]
    pub fn billing(mut self, billing: BillingFilter) -> Self {
        self.billing = billing;
        self
    }

    /// The same filters over another range.
    #[must_use]
    pub fn with_range(mut self, range: DateRange) -> Self {
        self.range = range;
        self
    }

    /// The range.
    #[must_use]
    pub const fn range(&self) -> DateRange {
        self.range
    }

    /// The billing restriction.
    #[must_use]
    pub const fn billing_filter(&self) -> &BillingFilter {
        &self.billing
    }

    /// The agents restriction (empty: all).
    #[must_use]
    pub fn agent_list(&self) -> &[AgentId] {
        &self.agents
    }

    /// Whether a profile restriction is set.
    #[must_use]
    pub const fn restricts_profiles(&self) -> bool {
        !self.profiles.is_empty()
    }

    /// The restricted profile names that apply to `agent` (empty when none or unrestricted).
    #[must_use]
    pub fn profiles_of(&self, agent: AgentId) -> Vec<ProfileId> {
        let mut out: Vec<ProfileId> = Vec::new();
        for s in self
            .profiles
            .iter()
            .filter(|s| s.agent.is_none_or(|a| a == agent))
        {
            if !out.contains(&s.profile) {
                out.push(s.profile.clone());
            }
        }
        out
    }

    /// Whether anything besides the range restricts the selection. When false, paid cost
    /// includes every subscription fee of the range, used or not (D28).
    #[must_use]
    pub fn is_scoped(&self) -> bool {
        !(self.agents.is_empty()
            && self.profiles.is_empty()
            && self.models.is_empty()
            && self.projects.is_empty()
            && self.clients.is_empty()
            && self.billing == BillingFilter::Any)
    }

    /// SQL conditions on `usage_event` (without the time range) and their parameters. Only
    /// static fragments and `?` placeholders are produced.
    pub(crate) fn sql(&self, sql: &mut String, params: &mut Vec<Value>) {
        self.sql_agent_profile(sql, params);
        list(sql, params, "model", &self.models);
        list(sql, params, "project", &self.projects);
        let clients: Vec<&str> = self.clients.iter().map(ClientName::as_str).collect();
        list(sql, params, "client", &clients);
    }

    /// Only the agent and profile conditions, for tables without model or project columns.
    pub(crate) fn sql_agent_profile(&self, sql: &mut String, params: &mut Vec<Value>) {
        self.sql_agents(sql, params);
        if self.profiles.is_empty() {
            return;
        }
        let terms: Vec<&str> = self
            .profiles
            .iter()
            .map(|s| {
                if let Some(agent) = s.agent {
                    params.push(Value::Text(agent.as_str().to_owned()));
                    params.push(Value::Text(s.profile.as_str().to_owned()));
                    "(agent = ? AND profile = ?)"
                } else {
                    params.push(Value::Text(s.profile.as_str().to_owned()));
                    "profile = ?"
                }
            })
            .collect();
        sql.push_str(" AND (");
        sql.push_str(&terms.join(" OR "));
        sql.push(')');
    }

    /// Only the agent condition, for tables without a profile column.
    pub(crate) fn sql_agents(&self, sql: &mut String, params: &mut Vec<Value>) {
        let agents: Vec<&str> = self.agents.iter().map(|a| a.as_str()).collect();
        list(sql, params, "agent", &agents);
    }
}

fn list<T: AsRef<str>>(sql: &mut String, params: &mut Vec<Value>, column: &str, v: &[T]) {
    if v.is_empty() {
        return;
    }
    sql.push_str(" AND ");
    sql.push_str(column);
    sql.push_str(" IN (");
    sql.push_str(&vec!["?"; v.len()].join(","));
    sql.push(')');
    params.extend(v.iter().map(|x| Value::Text(x.as_ref().to_owned())));
}

/// Untrusted filter input as the UI or CLI sends it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterSpec {
    /// The range; today when absent.
    pub range: Option<RangeSpec>,
    /// Agent ids.
    pub agents: Vec<String>,
    /// Profiles: `agent/profile`, or a bare profile name matching it under any agent.
    pub profiles: Vec<String>,
    /// Model ids.
    pub models: Vec<String>,
    /// Project keys.
    pub projects: Vec<String>,
    /// Client labels.
    pub clients: Vec<String>,
    /// Billing restriction.
    pub billing: BillingFilter,
}

fn parse_all<T, E>(
    kind: &'static str,
    values: &[String],
    parse: impl Fn(&str) -> Result<T, E>,
) -> Result<Vec<T>, FilterError> {
    values
        .iter()
        .map(|v| {
            parse(v).map_err(|_| FilterError::BadValue {
                kind,
                value: v.clone(),
            })
        })
        .collect()
}

impl Filters {
    /// Parses untrusted input.
    ///
    /// # Errors
    ///
    /// [`FilterError`] for a bad range or identifier.
    pub fn parse(spec: &FilterSpec, ctx: &QueryContext) -> Result<Self, FilterError> {
        let range = spec.range.unwrap_or(RangeSpec::Today).resolve(ctx)?;
        Ok(Self {
            range,
            agents: parse_all("agent", &spec.agents, AgentId::from_str)?,
            profiles: parse_all("profile", &spec.profiles, ProfileSelector::from_str)?,
            models: parse_all("model", &spec.models, ModelId::from_str)?,
            projects: parse_all("project", &spec.projects, ProjectKey::from_str)?,
            clients: parse_all("client", &spec.clients, |v| {
                ClientName::try_from(v.to_owned())
            })?,
            billing: spec.billing.clone(),
        })
    }
}
