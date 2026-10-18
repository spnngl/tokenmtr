//! Arguments the UI sends, parsed at the boundary into typed values.

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use tokenmtr_app::{ReportFormat, ReportRange, ReportRequest};
use tokenmtr_core::Currency;
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
    /// Agent filter (`agent=` search param); must be a known agent id.
    pub agent: Option<String>,
}

impl DashboardTarget {
    /// The `#/route?agent=…` fragment of the main window URL.
    ///
    /// # Errors
    ///
    /// Returns the offending text when `agent` is not a valid agent id (so it cannot inject
    /// anything into the URL).
    pub fn fragment(&self) -> Result<String, String> {
        let Some(agent) = &self.agent else {
            return Ok(format!("#{}", self.route.path()));
        };
        let valid = !agent.is_empty()
            && agent
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !valid {
            return Err(format!("invalid agent id {agent:?}"));
        }
        Ok(format!("#{}?agent={agent}", self.route.path()))
    }
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
    fn fragment_carries_a_validated_agent() {
        let target = DashboardTarget {
            route: Route::Quota,
            agent: Some("claude".to_owned()),
        };
        assert_eq!(target.fragment().as_deref(), Ok("#/quota?agent=claude"));
        let bad = DashboardTarget {
            route: Route::Overview,
            agent: Some("a&b".to_owned()),
        };
        assert!(bad.fragment().is_err());
        let none = DashboardTarget {
            route: Route::Settings,
            agent: None,
        };
        assert_eq!(none.fragment().as_deref(), Ok("#/settings"));
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
