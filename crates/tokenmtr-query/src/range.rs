//! Calendar ranges and buckets in the configured zone (D8).

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use serde::{Deserialize, Serialize};

use crate::context::{QueryContext, WeekStart};
use crate::error::{FilterError, QueryError};

/// Local calendar days `[start, end)` in the context's zone. Never empty.
///
/// Calendar ranges, not instants, so DST days keep their real length (23 or 25 hours) and
/// subscription fees accrue per local day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRange")]
pub struct DateRange {
    start: Date,
    end: Date,
}

#[derive(Deserialize)]
struct RawRange {
    start: Date,
    end: Date,
}

impl TryFrom<RawRange> for DateRange {
    type Error = FilterError;
    fn try_from(raw: RawRange) -> Result<Self, FilterError> {
        Self::new(raw.start, raw.end)
    }
}

impl DateRange {
    /// A range of whole local days.
    ///
    /// # Errors
    ///
    /// [`FilterError::EmptyRange`] unless `start < end`.
    pub fn new(start: Date, end: Date) -> Result<Self, FilterError> {
        if start < end {
            Ok(Self { start, end })
        } else {
            Err(FilterError::EmptyRange)
        }
    }

    /// The range of the single day `day`.
    ///
    /// # Errors
    ///
    /// [`FilterError::OutOfRange`] past the last supported date.
    pub fn day(day: Date) -> Result<Self, FilterError> {
        Self::new(day, day.tomorrow().map_err(|_| FilterError::OutOfRange)?)
    }

    /// First day, inclusive.
    #[must_use]
    pub const fn start(&self) -> Date {
        self.start
    }

    /// Last day, exclusive.
    #[must_use]
    pub const fn end(&self) -> Date {
        self.end
    }

    /// Number of calendar days.
    #[must_use]
    pub fn days(&self) -> u32 {
        self.end
            .since(self.start)
            .ok()
            .and_then(|s| u32::try_from(s.get_days()).ok())
            .unwrap_or(0)
    }

    /// Every day of the range, in order.
    pub fn dates(&self) -> impl Iterator<Item = Date> + use<> {
        let end = self.end;
        std::iter::successors(Some(self.start), |d| d.tomorrow().ok()).take_while(move |d| *d < end)
    }

    /// Whether `day` lies inside.
    #[must_use]
    pub fn contains(&self, day: Date) -> bool {
        self.start <= day && day < self.end
    }

    /// The instants `[start of first day, start of the day after the last)` in `zone`.
    ///
    /// # Errors
    ///
    /// Time errors at the edge of the supported calendar.
    pub fn instants(&self, zone: &TimeZone) -> Result<(Timestamp, Timestamp), QueryError> {
        Ok((
            self.start.to_zoned(zone.clone())?.timestamp(),
            self.end.to_zoned(zone.clone())?.timestamp(),
        ))
    }
}

/// A calendar period of the recap and of budgets (D34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    /// The current local day.
    Today,
    /// The current week.
    Week,
    /// The current month.
    Month,
}

impl Period {
    /// The period containing `day`.
    ///
    /// # Errors
    ///
    /// [`FilterError::OutOfRange`] at the edge of the supported calendar.
    pub fn containing(self, day: Date, week_start: WeekStart) -> Result<DateRange, FilterError> {
        let out = |_| FilterError::OutOfRange;
        let (start, next) = match self {
            Self::Today => (day, day.tomorrow().map_err(out)?),
            Self::Week => {
                let start = Bucket::Week.start_of(day, week_start).map_err(out)?;
                (start, start.checked_add(Span::new().days(7)).map_err(out)?)
            }
            Self::Month => {
                let start = day.first_of_month();
                (
                    start,
                    start.checked_add(Span::new().months(1)).map_err(out)?,
                )
            }
        };
        DateRange::new(start, next)
    }
}

/// How a range is chosen, as the UI and CLI express it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RangeSpec {
    /// The current local day.
    Today,
    /// The last `days` local days, today included.
    LastDays {
        /// How many days, at least one.
        days: u16,
    },
    /// The current week.
    ThisWeek,
    /// The current month.
    ThisMonth,
    /// Explicit local days; `end` is exclusive.
    Custom {
        /// First day.
        start: Date,
        /// Day after the last.
        end: Date,
    },
}

impl RangeSpec {
    /// Resolves the spec against the context's clock, zone and week start.
    ///
    /// # Errors
    ///
    /// [`FilterError`] for empty or out-of-calendar ranges.
    pub fn resolve(self, ctx: &QueryContext) -> Result<DateRange, FilterError> {
        let today = ctx.today();
        match self {
            Self::Today => Period::Today.containing(today, ctx.week_start()),
            Self::ThisWeek => Period::Week.containing(today, ctx.week_start()),
            Self::ThisMonth => Period::Month.containing(today, ctx.week_start()),
            Self::LastDays { days } => {
                if days == 0 {
                    return Err(FilterError::ZeroDays);
                }
                let start = today
                    .checked_sub(Span::new().days(i64::from(days) - 1))
                    .map_err(|_| FilterError::OutOfRange)?;
                DateRange::new(
                    start,
                    today.tomorrow().map_err(|_| FilterError::OutOfRange)?,
                )
            }
            Self::Custom { start, end } => DateRange::new(start, end),
        }
    }
}

/// Size of a time-series bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    /// One local day.
    Day,
    /// One week starting on the configured day.
    Week,
    /// One calendar month.
    Month,
}

impl Bucket {
    /// The first day of the bucket containing `day`.
    ///
    /// # Errors
    ///
    /// Time errors at the edge of the supported calendar.
    pub fn start_of(self, day: Date, week_start: WeekStart) -> Result<Date, jiff::Error> {
        match self {
            Self::Day => Ok(day),
            Self::Month => Ok(day.first_of_month()),
            Self::Week => {
                let weekday = day.weekday();
                let back = match week_start {
                    WeekStart::Monday => weekday.to_monday_zero_offset(),
                    WeekStart::Sunday => weekday.to_sunday_zero_offset(),
                };
                day.checked_sub(Span::new().days(i64::from(back)))
            }
        }
    }

    /// The first day after the bucket that starts on `start`.
    ///
    /// # Errors
    ///
    /// Time errors at the edge of the supported calendar.
    pub fn next(self, start: Date) -> Result<Date, jiff::Error> {
        match self {
            Self::Day => start.tomorrow(),
            Self::Week => start.checked_add(Span::new().days(7)),
            Self::Month => start.first_of_month().checked_add(Span::new().months(1)),
        }
    }

    /// The smallest bucket that keeps `range` within `max_points` points (D47).
    #[must_use]
    pub fn pick(range: &DateRange, max_points: u32) -> Self {
        let days = range.days();
        if days <= max_points {
            Self::Day
        } else if days.div_ceil(7) < max_points {
            Self::Week
        } else {
            Self::Month
        }
    }
}
