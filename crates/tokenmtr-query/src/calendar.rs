//! Activity calendar: tokens and started sessions per local day over the last two years.

use std::collections::BTreeMap;

use jiff::Span;
use jiff::civil::Date;
use serde::Serialize;
use tokenmtr_store::StoreReader;

use crate::context::QueryContext;
use crate::engine::{Dim, Scope, billed_cells, session_starts};
use crate::error::QueryError;
use crate::filters::Filters;
use crate::range::{Bucket, DateRange};

/// Weeks an [`ActivityCalendar`] covers, the current one included: two years.
pub const CALENDAR_WEEKS: i64 = 105;

/// One local day of the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ActivityDay {
    /// Local day.
    pub day: Date,
    /// Processed tokens, daily aggregates included.
    pub total_tokens: u64,
    /// Sessions whose first event in the calendar falls on it (real traffic only).
    pub sessions: u64,
}

/// Daily activity, GitHub-contribution style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivityCalendar {
    /// Zero-filled, in order: from the first day of the week [`CALENDAR_WEEKS`] − 1 weeks
    /// before today's, to today.
    pub days: Vec<ActivityDay>,
}

/// Tokens and started sessions per local day of `filters`, over the calendar's own range: the
/// range of `filters` is ignored.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn activity_calendar(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<ActivityCalendar, QueryError> {
    let today = ctx.today();
    let start = Bucket::Week
        .start_of(today, ctx.week_start())?
        .checked_sub(Span::new().weeks(CALENDAR_WEEKS - 1))?;
    let range =
        DateRange::new(start, today.tomorrow()?).map_err(|_| QueryError::decode("range", today))?;
    let filters = filters.clone().with_range(range);
    let mut days: BTreeMap<Date, ActivityDay> = range
        .dates()
        .map(|day| {
            (
                day,
                ActivityDay {
                    day,
                    total_tokens: 0,
                    sessions: 0,
                },
            )
        })
        .collect();
    for (cell, local, _) in billed_cells(reader, ctx, &filters, Dim::None, Scope::Usage)? {
        if let Some(day) = days.get_mut(&local) {
            day.total_tokens += cell.total_tokens();
        }
    }
    for start in session_starts(reader, ctx, &filters)? {
        if let Some(day) = days.get_mut(&ctx.zone().to_datetime(start).date()) {
            day.sessions += 1;
        }
    }
    Ok(ActivityCalendar {
        days: days.into_values().collect(),
    })
}
