//! USD → EUR conversion and the ECB rate-selection rule (D27).

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::money::{EUR_PER_USD_SCALE, EurMicros, EurPerUsd, UsdMicros, saturating_micros};

/// A rate older than this many days is flagged approximate. The longest normal ECB gap is five
/// days (Thursday → Tuesday around Easter); anything longer means the fetch is failing.
pub const MAX_FRESH_GAP_DAYS: i32 = 5;

/// Converts micro-USD to micro-EUR, rounding half away from zero and saturating at the bounds.
#[must_use]
pub fn convert(usd: UsdMicros, rate: EurPerUsd) -> EurMicros {
    let numerator = i128::from(usd.get()) * i128::from(rate.scaled());
    let half = i128::from(EUR_PER_USD_SCALE / 2);
    let scale = i128::from(EUR_PER_USD_SCALE);
    let rounded = if numerator >= 0 {
        (numerator + half) / scale
    } else {
        (numerator - half) / scale
    };
    EurMicros::new(saturating_micros(rounded))
}

/// The UTC calendar date of an instant: the date D27 selects rates by.
#[must_use]
pub fn utc_date(at: Timestamp) -> Date {
    at.to_zoned(TimeZone::UTC).date()
}

/// The ECB reference rate for one date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FxRate {
    /// Publication date.
    pub date: Date,
    /// EUR per 1 USD.
    pub eur_per_usd: EurPerUsd,
}

/// The outcome of [`FxTable::select`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedRate {
    /// The rate to convert with.
    pub rate: EurPerUsd,
    /// The date that rate was published.
    pub date: Date,
    /// True when the rate is older than [`MAX_FRESH_GAP_DAYS`] (offline fallback: the last known
    /// rate stands in for rates that never arrived).
    pub approximate: bool,
}

/// Rates sorted by date, one per date (the last inserted wins). Sorting happens once, here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FxTable {
    rates: Vec<FxRate>,
}

impl FxTable {
    /// Sorts the rates by date and keeps one per date.
    #[must_use]
    pub fn new(rates: impl IntoIterator<Item = FxRate>) -> Self {
        let mut sorted: Vec<FxRate> = rates.into_iter().collect();
        sorted.sort_by_key(|r| r.date);
        let mut unique: Vec<FxRate> = Vec::with_capacity(sorted.len());
        for rate in sorted {
            match unique.last_mut() {
                Some(last) if last.date == rate.date => *last = rate,
                _ => unique.push(rate),
            }
        }
        Self { rates: unique }
    }

    /// The latest rate dated strictly before `event_date` (the event's UTC date), so the rate
    /// already exists when the event is ingested. `None` when no rate is that old.
    #[must_use]
    pub fn select(&self, event_date: Date) -> Option<SelectedRate> {
        let end = self.rates.partition_point(|r| r.date < event_date);
        let rate = self.rates.get(end.checked_sub(1)?)?;
        let gap_days = event_date.since(rate.date).ok()?.get_days();
        Some(SelectedRate {
            rate: rate.eur_per_usd,
            date: rate.date,
            approximate: gap_days > MAX_FRESH_GAP_DAYS,
        })
    }

    /// Number of dates held.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rates.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;
    use proptest::prelude::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn rate(d: Date, scaled: i64) -> Result<FxRate, crate::money::MoneyError> {
        Ok(FxRate {
            date: d,
            eur_per_usd: EurPerUsd::from_scaled(scaled)?,
        })
    }

    /// Fri 2026-05-01, Mon 2026-05-04, Wed 2026-05-06 (Tuesday is missing).
    fn table() -> Result<FxTable, crate::money::MoneyError> {
        Ok(FxTable::new([
            rate(date(2026, 5, 6), 930_000_000)?,
            rate(date(2026, 5, 1), 910_000_000)?,
            rate(date(2026, 5, 4), 920_000_000)?,
        ]))
    }

    fn picked(t: &FxTable, d: Date) -> Option<i64> {
        t.select(d).map(|s| s.rate.scaled())
    }

    #[test]
    fn convert_rounds_half_away_from_zero() -> TestResult {
        let half = EurPerUsd::from_scaled(500_000_000)?;
        assert_eq!(convert(UsdMicros::new(3), half).get(), 2);
        assert_eq!(convert(UsdMicros::new(-3), half).get(), -2);
        assert_eq!(
            convert(
                UsdMicros::new(1_000_000),
                EurPerUsd::from_scaled(920_000_000)?
            )
            .get(),
            920_000
        );
        assert_eq!(
            convert(UsdMicros::new(i64::MAX), EurPerUsd::from_scaled(i64::MAX)?).get(),
            i64::MAX
        );
        Ok(())
    }

    #[test]
    fn weekend_uses_friday_and_monday_uses_friday_too() -> TestResult {
        let t = table()?;
        assert_eq!(picked(&t, date(2026, 5, 2)), Some(910_000_000)); // Saturday
        assert_eq!(picked(&t, date(2026, 5, 3)), Some(910_000_000)); // Sunday
        // Strictly before: Monday's own rate is not used for Monday events.
        assert_eq!(picked(&t, date(2026, 5, 4)), Some(910_000_000));
        assert_eq!(picked(&t, date(2026, 5, 5)), Some(920_000_000));
        Ok(())
    }

    #[test]
    fn missing_date_falls_back_to_the_previous_rate() -> TestResult {
        let t = table()?;
        // Wednesday event: Tuesday has no rate, Monday's is the latest strictly before.
        assert_eq!(picked(&t, date(2026, 5, 6)), Some(920_000_000));
        assert_eq!(picked(&t, date(2026, 5, 7)), Some(930_000_000));
        Ok(())
    }

    #[test]
    fn no_rate_before_the_first_date() -> TestResult {
        let t = table()?;
        assert_eq!(picked(&t, date(2026, 5, 1)), None);
        assert_eq!(picked(&t, date(2020, 1, 1)), None);
        assert_eq!(picked(&FxTable::default(), date(2026, 5, 1)), None);
        Ok(())
    }

    #[test]
    fn stale_rates_are_flagged_approximate() -> TestResult {
        let t = table()?;
        assert_eq!(
            t.select(date(2026, 5, 11)).map(|s| s.approximate),
            Some(false)
        ); // 5 days
        assert_eq!(
            t.select(date(2026, 5, 12)).map(|s| s.approximate),
            Some(true)
        ); // 6 days
        assert_eq!(
            t.select(date(2026, 5, 12)).map(|s| s.date),
            Some(date(2026, 5, 6))
        );
        Ok(())
    }

    #[test]
    fn table_sorts_and_keeps_the_last_rate_per_date() -> TestResult {
        let t = FxTable::new([
            rate(date(2026, 5, 2), 1)?,
            rate(date(2026, 5, 1), 2)?,
            rate(date(2026, 5, 2), 3)?,
        ]);
        assert_eq!(t.len(), 2);
        assert_eq!(picked(&t, date(2026, 5, 3)), Some(3));
        Ok(())
    }

    #[test]
    fn utc_date_ignores_local_offsets() -> TestResult {
        let at: Timestamp = "2026-05-01T23:30:00-05:00".parse()?; // 04:30 UTC next day
        assert_eq!(utc_date(at), date(2026, 5, 2));
        Ok(())
    }

    proptest! {
        #[test]
        fn convert_is_monotonic(a in any::<i64>(), b in any::<i64>(), scaled in 1_i64..=i64::MAX) {
            let rate = EurPerUsd::from_scaled(scaled).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(convert(UsdMicros::new(lo), rate) <= convert(UsdMicros::new(hi), rate));
        }

        #[test]
        fn convert_is_antisymmetric(a in -1_000_000_000_000_i64..1_000_000_000_000, scaled in 1_i64..2_000_000_000) {
            let rate = EurPerUsd::from_scaled(scaled).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(convert(UsdMicros::new(a), rate).get(), -convert(UsdMicros::new(-a), rate).get());
        }
    }
}
