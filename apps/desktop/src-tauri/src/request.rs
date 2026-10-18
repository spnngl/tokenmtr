//! Arguments the UI sends, parsed at the boundary into typed values.

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use tokenmtr_app::{ReportFormat, ReportRange, ReportRequest};
use tokenmtr_core::{AgentId, Currency, SessionId};
use tokenmtr_query::FilterSpec;
use tokenmtr_report::Scope;

/// The `{ request }` argument of every filtered query: the dashboard filters plus the currency.
#[derive(Debug, Clone, Deserialize)]
pub struct QueryRequest {
    /// Range, agents, profiles, models, projects, clients and billing.
    pub filters: FilterSpec,
    /// Display currency.
    pub currency: Currency,
}

/// A page of the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Route {
    /// Overview.
    #[serde(rename = "/")]
    Overview,
    /// Quota.
    #[serde(rename = "/quota")]
    Quota,
    /// Billing.
    #[serde(rename = "/billing")]
    Billing,
    /// Sessions.
    #[serde(rename = "/sessions")]
    Sessions,
    /// Insights.
    #[serde(rename = "/insights")]
    Insights,
    /// Models.
    #[serde(rename = "/models")]
    Models,
    /// Projects.
    #[serde(rename = "/projects")]
    Projects,
    /// Reports.
    #[serde(rename = "/reports")]
    Reports,
    /// Export.
    #[serde(rename = "/export")]
    Export,
    /// Settings.
    #[serde(rename = "/settings")]
    Settings,
}

impl Route {
    /// The hash route, such as `/quota`.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Overview => "/",
            Self::Quota => "/quota",
            Self::Billing => "/billing",
            Self::Sessions => "/sessions",
            Self::Insights => "/insights",
            Self::Models => "/models",
            Self::Projects => "/projects",
            Self::Reports => "/reports",
            Self::Export => "/export",
            Self::Settings => "/settings",
        }
    }
}

/// Where `open_dashboard` goes: a page, optionally filtered to one agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DashboardTarget {
    /// The page.
    pub route: Route,
    /// Agent filter (`agent=` search param); a known agent id, so it cannot inject anything
    /// into the URL.
    pub agent: Option<AgentId>,
}

impl DashboardTarget {
    /// The `#/route?agent=…` fragment of the main window URL.
    #[must_use]
    pub fn fragment(&self) -> String {
        match self.agent {
            Some(agent) => format!("#{}?agent={agent}", self.route.path()),
            None => format!("#{}", self.route.path()),
        }
    }
}

/// What `reveal_path` shows in Finder: one of the places the app knows, never a path the
/// webview chose.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevealTarget {
    /// The config file.
    ConfigFile,
    /// The export sinks' default directory.
    ExportsDir,
    /// The transcript file of a session.
    Transcript {
        /// The session's agent.
        agent: AgentId,
        /// The session.
        session: SessionId,
    },
    /// The directory holding the transcript of a session.
    TranscriptFolder {
        /// The session's agent.
        agent: AgentId,
        /// The session.
        session: SessionId,
    },
}

/// The days of a report as the UI sends them.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RangeWire {
    /// The calendar month containing `date`.
    Month {
        /// Any day of the month.
        date: Date,
    },
    /// `first` to `last`, both inclusive.
    Custom {
        /// First day.
        first: Date,
        /// Last day.
        last: Date,
    },
}

/// What a report covers as the UI sends it.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeWire {
    /// Everything.
    All,
    /// One project key.
    Project {
        /// The key.
        project: String,
    },
    /// One client label.
    Client {
        /// The label.
        client: String,
    },
}

/// Report format on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatWire {
    /// Markdown.
    Markdown,
    /// CSV.
    Csv,
    /// HTML.
    Html,
}

/// The argument of `generate_report` and `save_report`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReportWire {
    /// Days covered.
    pub range: RangeWire,
    /// What is covered.
    pub scope: ScopeWire,
    /// Output format.
    pub format: FormatWire,
    /// Report currency.
    pub currency: Currency,
}

impl From<ReportWire> for ReportRequest {
    fn from(wire: ReportWire) -> Self {
        Self {
            range: match wire.range {
                RangeWire::Month { date } => ReportRange::Month(date),
                RangeWire::Custom { first, last } => ReportRange::Custom { first, last },
            },
            scope: match wire.scope {
                ScopeWire::All => Scope::All,
                ScopeWire::Project { project } => Scope::Project(project),
                ScopeWire::Client { client } => Scope::Client(client),
            },
            format: match wire.format {
                FormatWire::Markdown => ReportFormat::Markdown,
                FormatWire::Csv => ReportFormat::Csv,
                FormatWire::Html => ReportFormat::Html,
            },
            currency: Some(wire.currency),
        }
    }
}

impl From<ReportFormat> for FormatWire {
    fn from(format: ReportFormat) -> Self {
        match format {
            ReportFormat::Markdown => Self::Markdown,
            ReportFormat::Csv => Self::Csv,
            ReportFormat::Html => Self::Html,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_targets_parse_the_ui_shape() -> Result<(), serde_json::Error> {
        let target: RevealTarget = serde_json::from_str(r#"{"kind":"exports_dir"}"#)?;
        assert_eq!(target, RevealTarget::ExportsDir);
        let target: RevealTarget = serde_json::from_str(
            r#"{"kind":"transcript_folder","agent":"claude","session":"s1"}"#,
        )?;
        assert!(matches!(
            target,
            RevealTarget::TranscriptFolder { agent, .. } if agent == AgentId::CLAUDE
        ));
        assert!(serde_json::from_str::<RevealTarget>(r#"{"kind":"path","path":"/"}"#).is_err());
        Ok(())
    }

    #[test]
    fn fragment_carries_a_known_agent() {
        let target = DashboardTarget {
            route: Route::Quota,
            agent: Some(AgentId::CLAUDE),
        };
        assert_eq!(target.fragment(), "#/quota?agent=claude");
        let bad = serde_json::from_str::<DashboardTarget>(r#"{"route":"/","agent":"a&b"}"#);
        assert!(bad.is_err());
        let none = DashboardTarget {
            route: Route::Settings,
            agent: None,
        };
        assert_eq!(none.fragment(), "#/settings");
    }

    #[test]
    fn query_request_parses_the_ui_shape() -> Result<(), serde_json::Error> {
        let request: QueryRequest = serde_json::from_str(
            r#"{"filters":{"range":{"kind":"last_days","days":7},"agents":["claude"],
            "billing":{"subscription":null}},"currency":"EUR"}"#,
        )?;
        assert_eq!(request.currency, Currency::Eur);
        Ok(())
    }

    #[test]
    fn report_wire_converts() -> Result<(), serde_json::Error> {
        let wire: ReportWire = serde_json::from_str(
            r#"{"range":{"kind":"month","date":"2026-05-01"},"scope":{"kind":"client","client":"acme"},
            "format":"csv","currency":"USD"}"#,
        )?;
        let request = ReportRequest::from(wire);
        assert_eq!(request.scope, Scope::Client("acme".to_owned()));
        assert_eq!(request.currency, Some(Currency::Usd));
        Ok(())
    }
}
