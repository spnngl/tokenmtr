//! Paid versus API value over time: the billing charts.

use std::collections::BTreeMap;

use jiff::civil::{Date, DateTime};
use jiff::{SignedDuration, Span, Timestamp};
use serde::{Deserialize, Serialize};
use tokenmtr_core::{AgentId, Currency};
use tokenmtr_store::StoreReader;

use crate::context::QueryContext;
use crate::engine::{Dim, Mode, Scope, fetch, split};
use crate::error::QueryError;
use crate::filters::Filters;

/// Bucket size of a [`BillingSeries`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    /// One local clock hour (a DST day has 23 or 25 of them).
    Hourly,
    /// One local day.
    Daily,
    /// One calendar month.
    Monthly,
}

/// The money of a bucket, or of one agent's share of it, in the series currency.
///
/// `paid_micros` = `subscription_fee_micros` + `extra_usage_micros` + `pay_as_you_go_micros`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BillingAmounts {
    /// What the usage would cost at API prices (extra usage has none).
    pub api_value_micros: i64,
    /// What was actually paid.
    pub paid_micros: i64,
    /// Prorated subscription fees inside `paid_micros`.
    pub subscription_fee_micros: i64,
    /// Claude extra usage inside `paid_micros`.
    pub extra_usage_micros: i64,
    /// API cost of events outside any subscription inside `paid_micros`.
    pub pay_as_you_go_micros: i64,
}

impl BillingAmounts {
    fn add(&mut self, other: &Self) {
        self.api_value_micros += other.api_value_micros;
        self.paid_micros += other.paid_micros;
        self.subscription_fee_micros += other.subscription_fee_micros;
        self.extra_usage_micros += other.extra_usage_micros;
        self.pay_as_you_go_micros += other.pay_as_you_go_micros;
    }

    const fn fee(micros: i64) -> Self {
        Self {
            api_value_micros: 0,
            paid_micros: micros,
            subscription_fee_micros: micros,
            extra_usage_micros: 0,
            pay_as_you_go_micros: 0,
        }
    }
}

/// One bucket of a [`BillingSeries`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BillingPoint {
    /// Start of the bucket.
    pub start: Timestamp,
    /// The same start on the local clock of the context zone (an autumn DST day shows the
    /// repeated hour twice; `start` tells them apart).
    pub local: DateTime,
    /// Real length of the bucket in minutes.
    pub minutes: i64,
    /// Everything in the bucket.
    #[serde(flatten)]
    pub amounts: BillingAmounts,
    /// `api_value_micros - paid_micros`; negative when paying more than the API value.
    pub savings_micros: i64,
    /// `api_value_micros / paid_micros`; absent when nothing was paid.
    pub roi: Option<f64>,
    /// Running API value since the first bucket.
    pub cumulative_api_value_micros: i64,
    /// Running paid cost since the first bucket.
    pub cumulative_paid_micros: i64,
    /// Per agent id when asked for; idle subscription fees (days without covered usage) are
    /// in the bucket only.
    pub agents: BTreeMap<String, BillingAmounts>,
    /// True when an estimate, unpriced event or non-fresh FX rate contributed.
    pub approximate: bool,
}

/// A zero-filled, contiguous series of paid versus API value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BillingSeries {
    /// Currency of every money field.
    pub currency: Currency,
    /// Bucket size.
    pub granularity: Granularity,
    /// Buckets in time order, empty ones included.
    pub points: Vec<BillingPoint>,
}

/// Paid versus API value of `filters` per bucket, covering every bucket that touches the range.
///
/// Money follows the billing rules of [`crate::billing`]: subscription fees accrue per local day
/// (until today) and are shared among the day's covered events; extra usage is paid only. Hourly
/// buckets spread each day's fee evenly over the day's hours (23 or 25 on a DST day), so the
/// hours of a day sum to its daily fee; a day's API value and pay-as-you-go cost stay in the hour
/// they happened.
///
/// # Errors
///
/// Store, SQL or decode failures; EUR fees without any rate.
pub fn billing_series(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    granularity: Granularity,
    split_by_agent: bool,
) -> Result<BillingSeries, QueryError> {
    let fetched = fetch(reader, ctx, filters, Dim::None, Scope::All)?;
    let mut points = empty_points(ctx, filters, granularity)?;
    // Fees per local day (and per agent), spread over the day's buckets at the end.
    let mut fees: BTreeMap<Date, (i64, BTreeMap<AgentId, i64>)> = BTreeMap::new();
    for row in &fetched.rows {
        let Some(point) = index_of(&points, row.cell.start).and_then(|i| points.get_mut(i)) else {
            continue;
        };
        let spent = row.paid - row.fee;
        let amounts = BillingAmounts {
            api_value_micros: row.api,
            paid_micros: spent,
            subscription_fee_micros: 0,
            extra_usage_micros: if row.mode == Mode::Extra { spent } else { 0 },
            pay_as_you_go_micros: if row.mode == Mode::Api { spent } else { 0 },
        };
        point.amounts.add(&amounts);
        point.approximate |= row.approximate;
        if split_by_agent {
            point
                .agents
                .entry(row.cell.agent.as_str().to_owned())
                .or_default()
                .add(&amounts);
        }
        if row.fee != 0 {
            let day = fees.entry(row.local).or_default();
            day.0 += row.fee;
            *day.1.entry(row.cell.agent).or_default() += row.fee;
        }
    }
    for idle in &fetched.idle {
        fees.entry(idle.local).or_default().0 += idle.paid;
        if let Some(point) =
            index_of(&points, day_start(ctx, idle.local)?).and_then(|i| points.get_mut(i))
        {
            point.approximate |= idle.approximate;
        }
    }
    for (day, (fee, per_agent)) in fees {
        let buckets = day_buckets(
            &points,
            day_start(ctx, day)?,
            day_start(ctx, day.tomorrow()?)?,
        );
        let spread = |micros: i64| split(micros, &vec![1; buckets.len()], signed_len(&buckets));
        for (i, part) in buckets.clone().zip(spread(fee)) {
            if let Some(point) = points.get_mut(i) {
                point.amounts.add(&BillingAmounts::fee(part));
            }
        }
        if !split_by_agent {
            continue;
        }
        for (agent, micros) in per_agent {
            for (i, part) in buckets.clone().zip(spread(micros)) {
                if let Some(point) = points.get_mut(i) {
                    point
                        .agents
                        .entry(agent.as_str().to_owned())
                        .or_default()
                        .add(&BillingAmounts::fee(part));
                }
            }
        }
    }
    let (mut api, mut paid) = (0, 0);
    for point in &mut points {
        let a = point.amounts;
        api += a.api_value_micros;
        paid += a.paid_micros;
        point.savings_micros = a.api_value_micros - a.paid_micros;
        point.roi = roi(a.api_value_micros, a.paid_micros);
        point.cumulative_api_value_micros = api;
        point.cumulative_paid_micros = paid;
    }
    Ok(BillingSeries {
        currency: ctx.currency(),
        granularity,
        points,
    })
}

/// The buckets touching the range of `filters`, all zero.
fn empty_points(
    ctx: &QueryContext,
    filters: &Filters,
    granularity: Granularity,
) -> Result<Vec<BillingPoint>, QueryError> {
    let range = filters.range();
    let mut bounds = Vec::new();
    match granularity {
        Granularity::Hourly => {
            // Zone offsets change by whole hours, so absolute hours from local midnight are local
            // clock hours.
            let end = day_start(ctx, range.end())?;
            let mut at = day_start(ctx, range.start())?;
            while at < end {
                let next = at.checked_add(SignedDuration::from_hours(1))?;
                bounds.push((at, next));
                at = next;
            }
        }
        Granularity::Daily => {
            for day in range.dates() {
                bounds.push((day_start(ctx, day)?, day_start(ctx, day.tomorrow()?)?));
            }
        }
        Granularity::Monthly => {
            let mut month = range.start().first_of_month();
            while month < range.end() {
                let next = month.checked_add(Span::new().months(1))?;
                bounds.push((day_start(ctx, month)?, day_start(ctx, next)?));
                month = next;
            }
        }
    }
    let mut points = Vec::with_capacity(bounds.len());
    for (start, next) in bounds {
        points.push(BillingPoint {
            start,
            local: ctx.zone().to_datetime(start),
            minutes: next.duration_since(start).as_mins(),
            amounts: BillingAmounts::default(),
            savings_micros: 0,
            roi: None,
            cumulative_api_value_micros: 0,
            cumulative_paid_micros: 0,
            agents: BTreeMap::new(),
            approximate: false,
        });
    }
    Ok(points)
}

/// The first instant of local `day`.
fn day_start(ctx: &QueryContext, day: Date) -> Result<Timestamp, QueryError> {
    Ok(day.to_zoned(ctx.zone().clone())?.timestamp())
}

/// Index of the bucket holding instant `at`.
fn index_of(points: &[BillingPoint], at: Timestamp) -> Option<usize> {
    points.partition_point(|p| p.start <= at).checked_sub(1)
}

/// The buckets a day `[from, to)` shares its fee with: its hours, or the one bucket holding it.
fn day_buckets(points: &[BillingPoint], from: Timestamp, to: Timestamp) -> std::ops::Range<usize> {
    let Some(first) = index_of(points, from) else {
        return 0..0;
    };
    let last = points.partition_point(|p| p.start < to);
    first..last.max(first + 1)
}

fn signed_len(range: &std::ops::Range<usize>) -> i64 {
    i64::try_from(range.len()).unwrap_or(i64::MAX)
}

/// `api / paid`; absent when nothing was paid.
#[expect(
    clippy::cast_precision_loss,
    reason = "display ratio of micros far below 2^53"
)]
fn roi(api: i64, paid: i64) -> Option<f64> {
    (paid != 0).then(|| api as f64 / paid as f64)
}
