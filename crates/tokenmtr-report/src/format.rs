//! Shared text formatting for the renderers.

use jiff::Timestamp;
use rust_decimal::{Decimal, RoundingStrategy};
use tokenmtr_core::Currency;

use crate::input::{Amount, Scope};

/// Marker shown before a figure that includes approximate events.
pub(crate) const APPROXIMATE_MARK: &str = "~";

const fn symbol(currency: Currency) -> &'static str {
    match currency {
        Currency::Usd => "$",
        Currency::Eur => "€",
    }
}

/// The ISO code of a currency.
pub(crate) const fn code(currency: Currency) -> &'static str {
    match currency {
        Currency::Usd => "USD",
        Currency::Eur => "EUR",
    }
}

fn decimal(amount: Amount) -> Decimal {
    Decimal::from_i128_with_scale(i128::from(amount.micros()), 6)
}

/// `$12.34`, with a leading `~` when `approximate`. Rounds half away from zero.
pub(crate) fn display_amount(amount: Amount, currency: Currency, approximate: bool) -> String {
    let rounded = decimal(amount).round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    let mark = if approximate { APPROXIMATE_MARK } else { "" };
    format!("{mark}{}{rounded:.2}", symbol(currency))
}

/// The exact amount with six decimals, for machine readers.
pub(crate) fn exact_amount(amount: Amount) -> String {
    format!("{:.6}", decimal(amount))
}

/// Human title of the scope.
pub(crate) fn scope_label(scope: &Scope) -> String {
    match scope {
        Scope::All => "All projects".to_owned(),
        Scope::Project(name) => format!("Project {name}"),
        Scope::Client(name) => format!("Client {name}"),
    }
}

/// An instant at second precision, UTC.
pub(crate) fn instant(at: Timestamp) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}
