//! The billing screen: subscriptions, paid versus API value, ROI (D28, D46).

use std::collections::BTreeMap;

use jiff::civil::Date;
use serde::Serialize;
use tokenmtr_core::{BillingPeriod, Currency, Plan};
use tokenmtr_store::StoreReader;

use crate::context::QueryContext;
use crate::engine::{Dim, Mode, Scope, fetch};
use crate::error::QueryError;
use crate::filters::Filters;
use crate::usage::Usage;

/// One declared subscription and what it covered in the range.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubscriptionLine {
    /// Subscription id.
    pub id: String,
    /// Provider.
    pub provider: String,
    /// Plan.
    pub plan: Plan,
    /// List fee per period, in its own currency.
    pub fee_micros: i64,
    /// Currency of `fee_micros`.
    pub fee_currency: Currency,
    /// Billing period.
    pub period: BillingPeriod,
    /// First active day.
    pub start: Date,
    /// First day no longer covered.
    pub end: Option<Date>,
    /// Covered usage; `subscription_fee_micros` is the prorated fee of the range in the result
    /// currency.
    pub usage: Usage,
    /// API value over fees, absent when no fee accrued in the range.
    pub roi: Option<f64>,
}

/// One calendar month of paid versus API value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BillingMonth {
    /// First day of the month (local).
    pub month: Date,
    /// Everything of that month.
    #[serde(flatten)]
    pub usage: Usage,
    /// Subscription ROI of the month.
    pub roi: Option<f64>,
}

/// Paid versus API value for `filters`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BillingReport {
    /// Currency of the money fields.
    pub currency: Currency,
    /// Everything: `paid_micros` = pay-as-you-go cost + prorated fees (D28).
    pub total: Usage,
    /// Pay-as-you-go usage, outside any subscription.
    pub api: Usage,
    /// Per declared subscription (all of them, also when idle).
    pub subscriptions: Vec<SubscriptionLine>,
    /// Per local month, zero months omitted.
    pub months: Vec<BillingMonth>,
    /// Subscription ROI of the range.
    pub roi: Option<f64>,
}

/// The billing view of `filters`.
///
/// # Errors
///
/// Store, SQL or decode failures; EUR fees without any rate.
pub fn billing(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<BillingReport, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::None, Scope::All)?;
    let mut total = Usage::default();
    let mut api = Usage::default();
    let mut per_sub: Vec<Usage> = vec![Usage::default(); ctx.subscriptions().len()];
    let mut months: BTreeMap<Date, Usage> = BTreeMap::new();
    for row in &fetched.rows {
        total.add(row);
        months
            .entry(row.local.first_of_month())
            .or_default()
            .add(row);
        match row.mode {
            Mode::Api => api.add(row),
            Mode::Subscription(i) => {
                if let Some(usage) = per_sub.get_mut(i) {
                    usage.add(row);
                }
            }
        }
    }
    for idle in &fetched.idle {
        total.add_idle(idle);
        months
            .entry(idle.local.first_of_month())
            .or_default()
            .add_idle(idle);
        if let Some(usage) = per_sub.get_mut(idle.sub) {
            usage.add_idle(idle);
        }
    }
    let subscriptions = ctx
        .subscriptions()
        .iter()
        .zip(per_sub)
        .map(|(sub, usage)| SubscriptionLine {
            id: sub.id().as_str().to_owned(),
            provider: sub.provider().as_str().to_owned(),
            plan: sub.plan(),
            fee_micros: sub.fee().micros(),
            fee_currency: sub.fee().currency(),
            period: sub.period(),
            start: sub.start(),
            end: sub.end(),
            roi: usage.roi(),
            usage,
        })
        .collect();
    Ok(BillingReport {
        currency: ctx.currency(),
        roi: total.roi(),
        total,
        api,
        subscriptions,
        months: months
            .into_iter()
            .map(|(month, usage)| BillingMonth {
                month,
                roi: usage.roi(),
                usage,
            })
            .collect(),
    })
}
