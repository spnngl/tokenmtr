//! Domain model and pure logic of `TokenMtr`: usage events, money, pricing, billing, forecasts,
//! budgets, the plan advisor and alert rules. No IO: parsing bytes in, values out.

pub mod advisor;
pub mod alert;
pub mod billing;
pub mod brand;
pub mod budget;
pub mod event;
pub mod forecast;
pub mod fx;
pub mod ids;
pub mod money;
pub mod pricing;
pub mod quota;
#[cfg(test)]
mod testing;

pub use advisor::{Advice, PlanFit, Recommendation, Verdict, advise};
pub use alert::{Alert, AlertConfig, AlertInputs, AlertKind, evaluate_alerts};
pub use billing::{
    BillingMode, BillingPeriod, Plan, PlanCatalog, Subscription, SubscriptionSpec, billing_mode,
    prorated_fee,
};
pub use budget::{Budget, BudgetMeasure, BudgetPeriod, BudgetStatus, budget_status};
pub use event::{
    ApiCost, Capabilities, CostSource, Estimate, EventCost, EventKind, ParsedEvent, SignalKind,
    TokenCounts, ToolCall, UsageEvent, split_mcp_tool_name,
};
pub use forecast::{LiveRate, QuotaEta, live_rate, project_period, quota_eta};
pub use fx::{FxRate, FxTable, SelectedRate, convert};
pub use ids::{
    AgentId, BudgetId, DedupKey, EventId, IdError, MachineId, ModelId, ProfileId, ProjectKey,
    ProviderId, SessionId, SinkId, SourceFileId, SubscriptionId,
};
pub use money::{Currency, EurMicros, EurPerUsd, Money, MoneyError, UsdMicros};
pub use pricing::{Override, PricingCatalog, PricingError, PricingVersion, estimate};
pub use quota::{QuotaSnapshot, QuotaWindow, Utilization, WindowId};
