//! Time series per day, week or month in the configured zone.

use std::collections::BTreeMap;

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use tokenmtr_core::Currency;
use tokenmtr_store::StoreReader;

use crate::context::QueryContext;
use crate::engine::{Dim, Mode, Scope, fetch};
use crate::error::QueryError;
use crate::filters::Filters;
use crate::range::Bucket;
use crate::usage::Usage;

/// How a series splits each bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    /// No split.
    None,
    /// By agent id.
    Agent,
    /// By model id.
    Model,
    /// By project key (`(none)` when unresolved).
    Project,
}

/// Key of events without a project.
pub const NO_GROUP: &str = "(none)";

/// One group's share of a bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GroupValue {
    /// Processed tokens.
    pub total_tokens: u64,
    /// API value in the series currency.
    pub api_value_micros: i64,
    /// Paid cost in the series currency (fees, and extra usage outside the agent split, are
    /// not split by group).
    pub paid_micros: i64,
}

/// One bucket.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesPoint {
    /// First local day of the bucket.
    pub start: Date,
    /// Real length of the bucket in minutes (1380 or 1500 for a DST day).
    pub minutes: i64,
    /// Everything in the bucket.
    #[serde(flatten)]
    pub usage: Usage,
    /// Running API value since the first bucket.
    pub cumulative_api_value_micros: i64,
    /// Running paid cost since the first bucket.
    pub cumulative_paid_micros: i64,
    /// The split by [`GroupBy`]; empty for [`GroupBy::None`].
    pub groups: BTreeMap<String, GroupValue>,
}

/// A zero-filled, contiguous series.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Series {
    /// Currency of every money field.
    pub currency: Currency,
    /// Bucket size.
    pub bucket: Bucket,
    /// The split.
    pub group_by: GroupBy,
    /// Buckets in time order, empty ones included.
    pub points: Vec<SeriesPoint>,
}

/// Usage per bucket, covering every bucket that touches the range.
///
/// # Errors
///
/// Store, SQL or decode failures; EUR fees without any rate.
pub fn series(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    bucket: Bucket,
    group_by: GroupBy,
) -> Result<Series, QueryError> {
    let dim = match group_by {
        GroupBy::None | GroupBy::Agent => Dim::None,
        GroupBy::Model => Dim::Model,
        GroupBy::Project => Dim::Project,
    };
    let fetched = fetch(reader, ctx, filters, dim, Scope::All)?;
    let week_start = ctx.week_start();
    let range = filters.range();
    let mut points: BTreeMap<Date, SeriesPoint> = BTreeMap::new();
    let mut start = bucket.start_of(range.start(), week_start)?;
    while start < range.end() {
        let next = bucket.next(start)?;
        let minutes = (next.to_zoned(ctx.zone().clone())?.timestamp().as_second()
            - start.to_zoned(ctx.zone().clone())?.timestamp().as_second())
            / 60;
        points.insert(
            start,
            SeriesPoint {
                start,
                minutes,
                usage: Usage::default(),
                cumulative_api_value_micros: 0,
                cumulative_paid_micros: 0,
                groups: BTreeMap::new(),
            },
        );
        start = next;
    }
    for row in &fetched.rows {
        let key = bucket.start_of(row.local, week_start)?;
        let Some(point) = points.get_mut(&key) else {
            continue;
        };
        point.usage.add(row);
        let name = match group_by {
            GroupBy::None => continue,
            GroupBy::Agent => row.cell.agent.as_str().to_owned(),
            // Extra usage has no model or project: like idle fees, it is in the bucket only.
            GroupBy::Model | GroupBy::Project if row.mode == Mode::Extra => continue,
            GroupBy::Model | GroupBy::Project => {
                row.cell.k1.clone().unwrap_or_else(|| NO_GROUP.to_owned())
            }
        };
        let group = point.groups.entry(name).or_default();
        group.total_tokens += row.cell.total_tokens();
        group.api_value_micros += row.api;
        group.paid_micros += row.paid;
    }
    for idle in &fetched.idle {
        if let Some(point) = points.get_mut(&bucket.start_of(idle.local, week_start)?) {
            point.usage.add_idle(idle);
        }
    }
    let mut points: Vec<SeriesPoint> = points.into_values().collect();
    let (mut api, mut paid) = (0, 0);
    for point in &mut points {
        api += point.usage.api_value_micros;
        paid += point.usage.paid_micros;
        point.cumulative_api_value_micros = api;
        point.cumulative_paid_micros = paid;
    }
    Ok(Series {
        currency: ctx.currency(),
        bucket,
        group_by,
        points,
    })
}
