//! Domain model and pure logic of `TokenMtr`: usage events, money, pricing, billing, forecasts,
//! budgets, the plan advisor and alert rules. No IO: parsing bytes in, values out.

pub mod advisor;
pub mod alert;
pub mod billing;
pub mod budget;
pub mod dataset;
pub mod event;
pub mod extra_usage;
pub mod forecast;
pub mod fx;
pub mod ids;
pub mod money;
pub mod pricing;
pub mod quota;
#[cfg(test)]
mod testing;

/// Every product-name constant (the leaf crate `tokenmtr-brand`).
pub use tokenmtr_brand as brand;

pub use advisor::{Advice, PlanFit, Recommendation, Verdict, advise};
pub use alert::{Alert, AlertConfig, AlertInputs, AlertKind, evaluate_alerts};
pub use billing::{
    BillingMode, BillingPeriod, Plan, PlanCatalog, Subscription, SubscriptionSpec, billing_mode,
    prorated_fee,
};
pub use budget::{Budget, BudgetMeasure, BudgetPeriod, BudgetStatus, budget_status};
pub use dataset::Dataset;
pub use event::{
    ApiCost, Capabilities, CostSource, Estimate, EventCost, EventKind, ParsedEvent, PriceModifiers,
    SignalKind, TokenCounts, ToolCall, UsageEvent, split_mcp_tool_name,
};
pub use extra_usage::{EXTRA_USAGE, ExtraUsage};
pub use forecast::{LiveRate, QuotaEta, live_rate, project_period, quota_eta};
pub use fx::{FxRate, FxTable, SelectedRate, convert};
pub use ids::{
    AgentId, BudgetId, DedupKey, EventId, IdError, MachineId, ModelId, ProfileId, ProjectKey,
    ProviderId, SessionId, SinkId, SourceFileId, SubscriptionId,
};
pub use money::{Currency, EurMicros, EurPerUsd, Money, MoneyError, UsdMicros, saturating_micros};
pub use pricing::{
    Override, PriceQuery, PriceSource, Priced, PricingCatalog, PricingError, PricingVersion,
    estimate,
};
pub use quota::{QuotaSnapshot, QuotaWindow, Utilization, WindowId};
