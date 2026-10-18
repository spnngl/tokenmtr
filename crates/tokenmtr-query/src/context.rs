//! What every query needs besides the store: time zone, currency, FX, subscriptions, clock.

use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use tokenmtr_core::{
    Currency, EurPerUsd, FxRate, FxTable, PricingCatalog, SelectedRate, Subscription,
};
use tokenmtr_store::{StoreRead, StoreReader};

use crate::error::QueryError;

/// The first day of a week bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeekStart {
    /// ISO weeks.
    #[default]
    Monday,
    /// US-style weeks.
    Sunday,
}

/// How USD becomes EUR.
#[derive(Debug, Clone)]
pub struct FxPolicy(Inner);

#[derive(Debug, Clone)]
enum Inner {
    /// `fx.fixed_eur_per_usd`: overrides the ECB entirely.
    Fixed(EurPerUsd),
    /// ECB rates, and the oldest one for days before the history starts.
    Table(FxTable, Option<EurPerUsd>),
}

/// The rate chosen for one date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RateUse {
    pub rate: EurPerUsd,
    pub approximate: bool,
}

impl FxPolicy {
    /// One configured rate for every date.
    #[must_use]
    pub const fn fixed(rate: EurPerUsd) -> Self {
        Self(Inner::Fixed(rate))
    }

    /// ECB rates; an event uses the latest rate dated strictly before its UTC date.
    #[must_use]
    pub fn from_rates(rates: &[FxRate]) -> Self {
        let table = FxTable::new(rates.iter().copied());
        let earliest = rates.iter().min_by_key(|r| r.date).map(|r| r.eur_per_usd);
        Self(Inner::Table(table, earliest))
    }

    /// No rates at all: EUR cannot be displayed.
    #[must_use]
    pub fn none() -> Self {
        Self::from_rates(&[])
    }

    /// Whether any rate is known.
    #[must_use]
    pub fn is_available(&self) -> bool {
        match &self.0 {
            Inner::Fixed(_) => true,
            Inner::Table(table, _) => !table.is_empty(),
        }
    }

    /// The rate for `date` (the date to convert, as a UTC date for events). Dates before the
    /// whole history use the oldest rate and are flagged approximate (offline fallback).
    pub(crate) fn rate(&self, date: Date) -> Option<RateUse> {
        match &self.0 {
            Inner::Fixed(rate) => Some(RateUse {
                rate: *rate,
                approximate: false,
            }),
            Inner::Table(table, earliest) => match table.select(date) {
                Some(SelectedRate {
                    rate, approximate, ..
                }) => Some(RateUse { rate, approximate }),
                None => earliest.map(|rate| RateUse {
                    rate,
                    approximate: true,
                }),
            },
        }
    }
}

/// Everything a query needs besides the store and its filters.
#[derive(Debug, Clone)]
pub struct QueryContext {
    zone: TimeZone,
    week_start: WeekStart,
    requested_currency: Currency,
    currency: Currency,
    fx: FxPolicy,
    now: Timestamp,
    subscriptions: Vec<Subscription>,
    pricing: Option<PricingCatalog>,
}

impl QueryContext {
    /// A context. When EUR is requested but no rate is known, amounts fall back to USD and
    /// [`Self::currency_fallback`] is true; every result reports the currency it really used.
    #[must_use]
    pub fn new(
        zone: TimeZone,
        week_start: WeekStart,
        currency: Currency,
        fx: FxPolicy,
        now: Timestamp,
    ) -> Self {
        let effective = if currency == Currency::Eur && !fx.is_available() {
            Currency::Usd
        } else {
            currency
        };
        Self {
            zone,
            week_start,
            requested_currency: currency,
            currency: effective,
            fx,
            now,
            subscriptions: Vec::new(),
            pricing: None,
        }
    }

    /// Declares the subscriptions that decide billing mode and paid cost.
    #[must_use]
    pub fn with_subscriptions(mut self, subscriptions: Vec<Subscription>) -> Self {
        self.subscriptions = subscriptions;
        self
    }

    /// Supplies the pricing catalog used to value cache savings. Without it savings are absent.
    #[must_use]
    pub fn with_pricing(mut self, pricing: PricingCatalog) -> Self {
        self.pricing = Some(pricing);
        self
    }

    /// The same context reporting in `currency`, with no fallback (budgets have their own).
    pub(crate) fn in_currency(&self, currency: Currency) -> Self {
        let mut other = self.clone();
        other.requested_currency = currency;
        other.currency = currency;
        other
    }

    /// The bucketing time zone.
    #[must_use]
    pub const fn zone(&self) -> &TimeZone {
        &self.zone
    }

    /// The first day of weeks.
    #[must_use]
    pub const fn week_start(&self) -> WeekStart {
        self.week_start
    }

    /// The currency amounts are reported in.
    #[must_use]
    pub const fn currency(&self) -> Currency {
        self.currency
    }

    /// True when the requested currency could not be honoured (EUR without FX rates).
    #[must_use]
    pub fn currency_fallback(&self) -> bool {
        self.requested_currency != self.currency
    }

    /// The query clock.
    #[must_use]
    pub const fn now(&self) -> Timestamp {
        self.now
    }

    /// Today in the context's zone.
    #[must_use]
    pub fn today(&self) -> Date {
        self.zone.to_datetime(self.now).date()
    }

    /// The declared subscriptions.
    #[must_use]
    pub fn subscriptions(&self) -> &[Subscription] {
        &self.subscriptions
    }

    pub(crate) const fn fx(&self) -> &FxPolicy {
        &self.fx
    }

    pub(crate) const fn pricing(&self) -> Option<&PricingCatalog> {
        self.pricing.as_ref()
    }
}

/// The ECB rates in the store, oldest first.
///
/// # Errors
///
/// SQL failures or a stored rate that does not parse.
pub fn load_fx_rates(reader: &StoreReader) -> Result<Vec<FxRate>, QueryError> {
    let mut statement = reader
        .connection()
        .prepare_cached("SELECT date, eur_per_usd_scaled FROM fx_rate ORDER BY date")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.map(|row| {
        let (date, scaled) = row?;
        Ok(FxRate {
            date: Date::from_str(&date).map_err(|_| QueryError::decode("fx date", &date))?,
            eur_per_usd: EurPerUsd::from_scaled(scaled)?,
        })
    })
    .collect()
}

/// The subscriptions mirrored into the store, removed ones excluded. The daemon mirrors the
/// config, so this is the config's view as of the last reload; callers holding the config may
/// pass its list instead.
///
/// # Errors
///
/// Store failures.
pub fn load_subscriptions(reader: &StoreReader) -> Result<Vec<Subscription>, QueryError> {
    Ok(reader.active_subscriptions()?)
}
