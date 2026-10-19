//! The plain data a report is built from.
//!
//! A later phase fills it from `tokenmtr-query`. Every amount is already in the report's
//! currency; this crate never converts.

use jiff::Timestamp;
use jiff::civil::Date;
use tokenmtr_core::Currency;

/// A non-negative amount of money in millionths of the report currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Amount(u64);

impl Amount {
    /// No money.
    pub const ZERO: Self = Self(0);

    /// An amount of `micros` millionths.
    #[must_use]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// The amount in millionths.
    #[must_use]
    pub const fn micros(self) -> u64 {
        self.0
    }

    pub(crate) fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }
}

/// The days a report covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Period {
    /// Title of the period, e.g. `2026-05`.
    pub label: String,
    /// First day, inclusive.
    pub first: Date,
    /// Last day, inclusive.
    pub last: Date,
}

/// What the report is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every project.
    All,
    /// One project.
    Project(String),
    /// Every project of one client.
    Client(String),
}

/// API value one project drew from one subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionShare {
    /// Id of the subscription.
    pub subscription: String,
    /// API value of the project's events covered by it.
    pub api_value: Amount,
}

/// One project row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInput {
    /// Project name.
    pub name: String,
    /// Client label, if any.
    pub client: Option<String>,
    /// True when some of its events are approximate.
    pub approximate: bool,
    /// API value of the events no subscription covers; it is paid at API rates.
    pub pay_as_you_go: Amount,
    /// API value per covering subscription, one entry per subscription.
    pub subscriptions: Vec<SubscriptionShare>,
}

/// One model row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInput {
    /// Model id.
    pub model: String,
    /// Total tokens.
    pub tokens: u64,
    /// API value.
    pub api_value: Amount,
    /// True when some of its events are approximate.
    pub approximate: bool,
}

/// One day row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayInput {
    /// The day, in the configured time zone.
    pub date: Date,
    /// API value.
    pub api_value: Amount,
    /// True when some of its events are approximate.
    pub approximate: bool,
}

/// What a subscription cost in the period and who else used it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionInput {
    /// Id of the subscription.
    pub id: String,
    /// Fee for the period.
    pub paid: Amount,
    /// API value drawn from it by events outside the report's scope. It takes its share of
    /// `paid` out of the report, so a one-project report shows only that project's part.
    pub outside_scope: Amount,
}

/// Everything a report needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportInput {
    /// Days covered.
    pub period: Period,
    /// What the report is about.
    pub scope: Scope,
    /// Currency of every amount.
    pub currency: Currency,
    /// When the report was made.
    pub generated_at: Timestamp,
    /// Number of sessions in scope.
    pub sessions: u64,
    /// Rows per project.
    pub projects: Vec<ProjectInput>,
    /// Rows per model.
    pub models: Vec<ModelInput>,
    /// Rows per day.
    pub days: Vec<DayInput>,
    /// Subscriptions active in the period.
    pub subscriptions: Vec<SubscriptionInput>,
    /// Claude extra usage in the period: pay-as-you-go spend that belongs to no project, model or
    /// day row. Paid for [`Scope::All`] only; ignored for a project or client scope.
    pub extra_usage: Amount,
}
