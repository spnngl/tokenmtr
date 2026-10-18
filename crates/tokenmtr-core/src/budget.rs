//! Spending budgets (D34).

use serde::{Deserialize, Serialize};

use crate::forecast::project_period;
use crate::ids::BudgetId;
use crate::money::{Money, MoneyError};

/// The calendar unit a budget resets on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPeriod {
    /// Resets every day.
    Day,
    /// Resets every week.
    Week,
    /// Resets every month.
    Month,
}

/// What a budget counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMeasure {
    /// Money actually paid: API cost plus prorated subscription fees.
    Paid,
    /// What the usage would cost at API rates, subscription or not.
    ApiValue,
}

/// Why a budget was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("budget {0} must have a positive amount")]
pub struct BudgetError(pub BudgetId);

#[derive(Deserialize)]
struct BudgetSpec {
    id: BudgetId,
    period: BudgetPeriod,
    amount: Money,
    measure: BudgetMeasure,
}

/// A spending limit over a period. The amount is always positive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BudgetSpec")]
pub struct Budget {
    id: BudgetId,
    period: BudgetPeriod,
    amount: Money,
    measure: BudgetMeasure,
}

impl Budget {
    /// Validates a budget.
    ///
    /// # Errors
    ///
    /// [`BudgetError`] when `amount` is not positive.
    pub fn new(
        id: BudgetId,
        period: BudgetPeriod,
        amount: Money,
        measure: BudgetMeasure,
    ) -> Result<Self, BudgetError> {
        if amount.micros() <= 0 {
            return Err(BudgetError(id));
        }
        Ok(Self {
            id,
            period,
            amount,
            measure,
        })
    }

    /// Unique id.
    #[must_use]
    pub const fn id(&self) -> &BudgetId {
        &self.id
    }

    /// Reset period.
    #[must_use]
    pub const fn period(&self) -> BudgetPeriod {
        self.period
    }

    /// The limit.
    #[must_use]
    pub const fn amount(&self) -> Money {
        self.amount
    }

    /// What is counted against the limit.
    #[must_use]
    pub const fn measure(&self) -> BudgetMeasure {
        self.measure
    }
}

impl TryFrom<BudgetSpec> for Budget {
    type Error = BudgetError;
    fn try_from(spec: BudgetSpec) -> Result<Self, BudgetError> {
        Self::new(spec.id, spec.period, spec.amount, spec.measure)
    }
}

/// How a budget stands within its current period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetStatus {
    /// Spent so far this period.
    pub spent: Money,
    /// The limit.
    pub limit: Money,
    /// `spent / limit`, in whole percent (floored; 0 when `spent` is negative).
    pub percent: u32,
    /// Projected spend at the end of the period ([`project_period`]).
    pub projected: Money,
    /// `projected / limit`, in whole percent.
    pub projected_percent: u32,
}

fn percent_of(part: Money, whole: Money) -> u32 {
    let pct = i128::from(part.micros()).saturating_mul(100) / i128::from(whole.micros().max(1));
    u32::try_from(pct.max(0)).unwrap_or(u32::MAX)
}

/// Status of `budget` given what was `spent` this period and the per-day history in the same
/// measure (oldest first).
///
/// # Errors
///
/// [`MoneyError::CurrencyMismatch`] when `spent` or the history is not in the budget currency.
pub fn budget_status(
    budget: &Budget,
    spent: Money,
    daily_history: &[Money],
    remaining_days: u32,
) -> Result<BudgetStatus, MoneyError> {
    budget.amount.same_currency(spent)?;
    let projected = project_period(spent, daily_history, remaining_days)?;
    Ok(BudgetStatus {
        spent,
        limit: budget.amount,
        percent: percent_of(spent, budget.amount),
        projected,
        projected_percent: percent_of(projected, budget.amount),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;
    use crate::testing::TestResult;

    fn eur(units: i64) -> Money {
        Money::from_micros(Currency::Eur, units * 1_000_000)
    }

    fn budget(amount: Money) -> Result<Budget, Box<dyn std::error::Error>> {
        Ok(Budget::new(
            BudgetId::try_from("monthly")?,
            BudgetPeriod::Month,
            amount,
            BudgetMeasure::Paid,
        )?)
    }

    #[test]
    fn status_reports_percent_and_projection() -> TestResult {
        let b = budget(eur(100))?;
        let status = budget_status(&b, eur(40), &[eur(2), eur(2)], 10)?;
        assert_eq!(status.percent, 40);
        assert_eq!(status.projected, eur(60));
        assert_eq!(status.projected_percent, 60);
        let over = budget_status(&b, eur(150), &[], 5)?;
        assert_eq!((over.percent, over.projected_percent), (150, 150));
        Ok(())
    }

    #[test]
    fn mixed_currencies_are_refused() -> TestResult {
        let b = budget(eur(100))?;
        assert!(budget_status(&b, Money::from_micros(Currency::Usd, 1), &[], 1).is_err());
        assert!(budget_status(&b, eur(1), &[Money::from_micros(Currency::Usd, 1)], 1).is_err());
        Ok(())
    }

    #[test]
    fn non_positive_amounts_are_refused() {
        assert!(budget(eur(0)).is_err());
        assert!(budget(eur(-5)).is_err());
    }
}
