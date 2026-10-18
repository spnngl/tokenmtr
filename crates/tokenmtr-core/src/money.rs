//! Money as integer micro-units. No floats in sums.

use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

/// Fixed-point scale of [`EurPerUsd`]: 1e9.
pub const EUR_PER_USD_SCALE: i64 = 1_000_000_000;

const MICROS: i64 = 1_000_000;

/// Why a money value was rejected or an operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// A float was NaN or infinite.
    #[error("amount is not a finite number")]
    NotFinite,
    /// The value does not fit in an `i64` of micro-units.
    #[error("amount out of range")]
    OutOfRange,
    /// A rate was zero or negative.
    #[error("exchange rate must be positive")]
    NonPositiveRate,
    /// Two amounts in different currencies were combined.
    #[error("currency mismatch: {left:?} vs {right:?}")]
    CurrencyMismatch {
        /// Currency of the left operand.
        left: Currency,
        /// Currency of the right operand.
        right: Currency,
    },
}

/// A currency we display. USD is the only stored one; EUR is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    /// US dollar.
    Usd,
    /// Euro.
    Eur,
}

/// Rounds half away from zero to a whole number and converts to `i64`.
fn round_to_i64(value: Decimal) -> Result<i64, MoneyError> {
    value
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_i64()
        .ok_or(MoneyError::OutOfRange)
}

macro_rules! micros_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            /// Zero.
            pub const ZERO: Self = Self(0);

            /// Wraps a raw micro-unit count.
            #[must_use]
            pub const fn new(micros: i64) -> Self {
                Self(micros)
            }

            /// The raw micro-unit count.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }

            /// Addition that clamps at the `i64` bounds.
            #[must_use]
            pub const fn saturating_add(self, other: Self) -> Self {
                Self(self.0.saturating_add(other.0))
            }

            /// Converts a decimal amount in whole units, rounding half away from zero.
            ///
            /// # Errors
            ///
            /// [`MoneyError::OutOfRange`] when the micro-unit count overflows `i64`.
            pub fn from_decimal(units: Decimal) -> Result<Self, MoneyError> {
                let micros = units
                    .checked_mul(Decimal::from(MICROS))
                    .ok_or(MoneyError::OutOfRange)?;
                round_to_i64(micros).map(Self)
            }

            /// Converts a float amount in whole units (the pi boundary), rounding half away
            /// from zero.
            ///
            /// # Errors
            ///
            /// [`MoneyError::NotFinite`] for NaN/infinity, [`MoneyError::OutOfRange`] on overflow.
            pub fn from_f64(units: f64) -> Result<Self, MoneyError> {
                if !units.is_finite() {
                    return Err(MoneyError::NotFinite);
                }
                Self::from_decimal(Decimal::from_f64(units).ok_or(MoneyError::OutOfRange)?)
            }
        }

        impl std::iter::Sum for $name {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                iter.fold(Self::ZERO, Self::saturating_add)
            }
        }
    };
}

micros_newtype!(
    /// Micro-US-dollars (1e-6 USD), the stored currency.
    UsdMicros
);
micros_newtype!(
    /// Micro-euros (1e-6 EUR). Derived only, never stored.
    EurMicros
);

/// EUR per 1 USD, scaled by 1e9 ([`EUR_PER_USD_SCALE`]). Always positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct EurPerUsd(i64);

impl EurPerUsd {
    /// Wraps an already scaled rate.
    ///
    /// # Errors
    ///
    /// [`MoneyError::NonPositiveRate`] when `scaled <= 0`.
    pub const fn from_scaled(scaled: i64) -> Result<Self, MoneyError> {
        if scaled > 0 {
            Ok(Self(scaled))
        } else {
            Err(MoneyError::NonPositiveRate)
        }
    }

    /// Scales a decimal EUR-per-USD rate (e.g. `0.92`), rounding to 1e-9.
    ///
    /// # Errors
    ///
    /// [`MoneyError::NonPositiveRate`] for rates that round to zero or below,
    /// [`MoneyError::OutOfRange`] on overflow.
    pub fn from_decimal(eur_per_usd: Decimal) -> Result<Self, MoneyError> {
        let scaled = eur_per_usd
            .checked_mul(Decimal::from(EUR_PER_USD_SCALE))
            .ok_or(MoneyError::OutOfRange)?;
        Self::from_scaled(round_to_i64(scaled)?)
    }

    /// Inverts the ECB quote (USD per 1 EUR, e.g. `1.0850`) into EUR per 1 USD.
    ///
    /// # Errors
    ///
    /// [`MoneyError::NonPositiveRate`] when the quote is zero or negative.
    pub fn from_usd_per_eur(usd_per_eur: Decimal) -> Result<Self, MoneyError> {
        if usd_per_eur <= Decimal::ZERO {
            return Err(MoneyError::NonPositiveRate);
        }
        let inverse = Decimal::ONE
            .checked_div(usd_per_eur)
            .ok_or(MoneyError::OutOfRange)?;
        Self::from_decimal(inverse)
    }

    /// The rate scaled by 1e9.
    #[must_use]
    pub const fn scaled(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for EurPerUsd {
    type Error = MoneyError;
    fn try_from(scaled: i64) -> Result<Self, MoneyError> {
        Self::from_scaled(scaled)
    }
}

impl From<EurPerUsd> for i64 {
    fn from(rate: EurPerUsd) -> Self {
        rate.0
    }
}

/// Narrows an `i128` intermediate (a product or sum of micro-units) to a stored amount. Out of
/// range saturates toward the sign of `value`: an overflow never turns into zero or wraps.
#[must_use]
pub fn saturating_micros(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

/// An amount in a known currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Money {
    /// US dollars.
    Usd(UsdMicros),
    /// Euros.
    Eur(EurMicros),
}

impl Money {
    /// Builds an amount from raw micro-units of `currency`.
    #[must_use]
    pub const fn from_micros(currency: Currency, micros: i64) -> Self {
        match currency {
            Currency::Usd => Self::Usd(UsdMicros::new(micros)),
            Currency::Eur => Self::Eur(EurMicros::new(micros)),
        }
    }

    /// Zero in `currency`.
    #[must_use]
    pub const fn zero(currency: Currency) -> Self {
        Self::from_micros(currency, 0)
    }

    /// The currency of this amount.
    #[must_use]
    pub const fn currency(self) -> Currency {
        match self {
            Self::Usd(_) => Currency::Usd,
            Self::Eur(_) => Currency::Eur,
        }
    }

    /// The raw micro-unit count.
    #[must_use]
    pub const fn micros(self) -> i64 {
        match self {
            Self::Usd(v) => v.get(),
            Self::Eur(v) => v.get(),
        }
    }

    pub(crate) fn same_currency(self, other: Self) -> Result<(), MoneyError> {
        if self.currency() == other.currency() {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch {
                left: self.currency(),
                right: other.currency(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn usd_rounds_half_away_from_zero() -> TestResult {
        assert_eq!(UsdMicros::from_decimal("0.0000005".parse()?)?.get(), 1);
        assert_eq!(UsdMicros::from_decimal("0.0000004".parse()?)?.get(), 0);
        assert_eq!(UsdMicros::from_decimal("-0.0000005".parse()?)?.get(), -1);
        assert_eq!(UsdMicros::from_decimal("1.5".parse()?)?.get(), 1_500_000);
        Ok(())
    }

    #[test]
    fn usd_from_f64_handles_boundaries() -> TestResult {
        assert_eq!(UsdMicros::from_f64(0.012_345_6)?.get(), 12_346);
        assert_eq!(UsdMicros::from_f64(f64::NAN), Err(MoneyError::NotFinite));
        assert_eq!(
            UsdMicros::from_f64(f64::INFINITY),
            Err(MoneyError::NotFinite)
        );
        assert_eq!(UsdMicros::from_f64(1e30), Err(MoneyError::OutOfRange));
        Ok(())
    }

    #[test]
    fn eur_per_usd_scales_and_inverts() -> TestResult {
        assert_eq!(
            EurPerUsd::from_decimal("0.92".parse()?)?.scaled(),
            920_000_000
        );
        assert_eq!(
            EurPerUsd::from_usd_per_eur("1.25".parse()?)?.scaled(),
            800_000_000
        );
        assert_eq!(EurPerUsd::from_scaled(0), Err(MoneyError::NonPositiveRate));
        assert!(serde_json::from_str::<EurPerUsd>("-5").is_err());
        Ok(())
    }

    #[test]
    fn money_refuses_mixed_currencies() {
        let usd = Money::from_micros(Currency::Usd, 5);
        let eur = Money::from_micros(Currency::Eur, 5);
        assert!(usd.same_currency(eur).is_err());
        assert_eq!(usd.same_currency(usd), Ok(()));
    }

    #[test]
    fn micros_saturate_instead_of_wrapping() {
        assert_eq!(saturating_micros(i128::from(i64::MAX) + 1), i64::MAX);
        assert_eq!(saturating_micros(i128::from(i64::MIN) - 1), i64::MIN);
        assert_eq!(saturating_micros(-5), -5);
    }
}
