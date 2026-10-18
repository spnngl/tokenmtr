//! Read-side aggregation over the `TokenMtr` store.
//!
//! Every function is pure over a [`tokenmtr_store::StoreReader`] and a
//! [`QueryContext`]. Nothing here writes. All SQL binds user input as
//! parameters; untrusted strings enter only through [`Filters::parse`],
//! which turns a [`FilterSpec`] into typed ids.
//!
//! # Inputs
//!
//! - [`QueryContext`]: IANA zone, week start, display currency, [`FxPolicy`],
//!   `now`, subscriptions, optional pricing catalog. Build it once per request.
//!   [`load_fx_rates`] and [`load_subscriptions`] read the store's copies.
//! - [`Filters`]: a [`DateRange`] (end exclusive, local civil days) plus agent,
//!   profile, model, project, client and [`BillingFilter`] restrictions.
//! - Callers map config into [`BudgetSpec`], [`SinkSpec`] and [`RecapConfig`];
//!   this crate does not depend on `tokenmtr-config`.
//!
//! # Time (D8)
//!
//! Events are grouped in SQL by 15-minute UTC cell, then bucketed in Rust by the
//! context zone. Zone offsets are multiples of 15 minutes, so a cell never
//! straddles a local hour. Day buckets on a DST change are 23 or 25 hours:
//! [`SeriesPoint::minutes`] carries the real length. Heatmap weekdays are
//! `1` = Monday … `7` = Sunday; hours are local.
//!
//! # Money (D10, D11, D27, D28, D46)
//!
//! - API value is `reported ?? estimated`; events with neither count as
//!   `unpriced_events` and contribute zero.
//! - Money fields are integer micros (`*_micros`) in the root `currency`
//!   (`"USD"` or `"EUR"`). Asking for EUR without any rate falls back to USD and
//!   sets `currency_fallback`. `api_value_usd_micros` always stays in USD.
//! - EUR conversion uses, per event, the latest rate dated strictly before the
//!   event's UTC date. A rate older than five days, or a date before all
//!   rates, sets `approximate`.
//! - `approximate` is true when any estimate, unpriced event, imported daily
//!   aggregate or non-fresh rate contributed.
//! - `paid_micros` is the API value of events outside any subscription plus the
//!   subscription fee prorated per local day ([`tokenmtr_core::prorated_fee`]),
//!   charged until today only. A day's fee is shared among that subscription's
//!   events by API-value share, so any breakdown sums to the total. Idle fee days
//!   appear only in unscoped views ([`Filters::is_scoped`] is false).
//!   `roi` is subscription API value over the fees allocated.
//! - Session and heatmap views skip daily aggregates (D38) and carry no fees.
//!
//! # JSON shapes
//!
//! All result types derive `Serialize` with `snake_case` fields. Timestamps are
//! RFC 3339 strings, dates are `YYYY-MM-DD`. Usage-bearing rows flatten
//! [`Usage`]: `events`, token counts (`input_uncached`, `cache_read`,
//! `cache_write_5m`, `cache_write_1h`, `output`, `reasoning`, `total_tokens`),
//! `api_value_micros`, `api_value_usd_micros`, `paid_micros`,
//! `subscription_value_micros`, `subscription_fee_micros`, `reported_events`,
//! `estimated_events`, `unpriced_events`, `approximate`. Entry points:
//!
//! | Function | Result |
//! |---|---|
//! | [`overview`] | [`Totals`]: usage + `cached_input_percent`, `cache_savings_micros`, `roi`, `sessions` |
//! | [`series`] | [`Series`]: contiguous, zero-filled [`SeriesPoint`]s, optional `groups` |
//! | [`models`], [`projects`], [`branches`], [`unresolved_cwds`] | rows with usage |
//! | [`sessions`], [`session_detail`] | [`SessionPage`], [`SessionDetail`] |
//! | [`tools`], [`signals`], [`heatmap`] | [`ToolsReport`], [`SignalsReport`], [`Heatmap`] |
//! | [`billing`] | [`BillingReport`]: total, api, per-subscription lines, months |
//! | [`quota_latest`], [`quota_history`], [`advisor`] | [`QuotaMeter`], [`QuotaHistory`], [`AdvisorReport`] |
//! | [`budgets`] | [`BudgetView`]: spent, limit, percent, projected |
//! | [`recap`], [`status`] | [`Recap`], [`Status`] |
//! | [`sink_health`], [`daemon_health`], [`ingest_status`], [`parse_issues`], [`alerts`], [`backup_runs`], [`fx_rates`] | health views |
//!
//! [`Recap`] and [`Status`] carry `status_schema` = [`STATUS_SCHEMA`] (`1`),
//! `generated_at`, and `system` (`level` of `ok`/`warning`/`error`, `daemon`,
//! `sinks`). Bump the schema on any breaking change. Sink health is tagged by
//! `state`: `healthy`, `lagging`, `failing` (with `since`, `last_error`) or
//! `disabled`.

mod billing;
mod breakdowns;
mod context;
mod engine;
mod error;
mod filters;
mod health;
mod quota;
mod range;
mod recap;
mod series;
mod usage;

pub use billing::{BillingMonth, BillingReport, SubscriptionLine, billing};
pub use breakdowns::{
    BranchRow, HeatCell, Heatmap, ModelRow, ProjectRow, SessionCompactions, SessionDetail,
    SessionEvent, SessionPage, SessionRow, SessionToolCall, SignalDay, SignalsReport, ToolRow,
    ToolsReport, Totals, UnresolvedCwd, branches, heatmap, models, overview, projects,
    session_detail, sessions, signals, tools, unresolved_cwds,
};
pub use context::{FxPolicy, QueryContext, WeekStart, load_fx_rates, load_subscriptions};
pub use error::{FilterError, QueryError};
pub use filters::{BillingFilter, ClientName, FilterSpec, Filters};
pub use health::{
    AlertView, BackupView, DaemonHealth, DaemonState, FxRateView, IngestStatus, ParseIssueView,
    SinkHealth, SinkRunView, SinkSpec, SinkState, alerts, backup_runs, daemon_health, fx_rates,
    ingest_status, parse_issues, sink_health,
};
pub use quota::{
    AdvisorReport, QuotaHistory, QuotaMeter, QuotaPoint, QuotaWindowView, advisor, quota_history,
    quota_latest,
};
pub use range::{Bucket, DateRange, Period, RangeSpec};
pub use recap::{
    AgentRecap, BudgetScope, BudgetSpec, BudgetView, HealthLevel, Live, Recap, RecapConfig,
    STATUS_SCHEMA, Selection, SparkPoint, Status, SystemHealth, budgets, recap, status,
};
pub use series::{GroupBy, GroupValue, NO_GROUP, Series, SeriesPoint, series};
pub use usage::Usage;

#[cfg(test)]
mod tests;
