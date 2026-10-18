//! Shared output helpers: JSON, money, sizes and durations.

use anyhow::Context;
use jiff::Timestamp;
use serde::Serialize;
use tokenmtr_core::Currency;

/// Prints `value` as pretty JSON on stdout.
pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(value).context("cannot encode the JSON output")?;
    println!("{text}");
    Ok(())
}

/// `12.34 USD` from micros, rounded half away from zero to cents.
pub fn money(micros: i64, currency: Currency) -> String {
    let unit = match currency {
        Currency::Usd => "USD",
        Currency::Eur => "EUR",
    };
    let cents = (i128::from(micros.unsigned_abs()) + 5_000) / 10_000;
    let sign = if micros < 0 && cents > 0 { "-" } else { "" };
    format!("{sign}{}.{:02} {unit}", cents / 100, cents % 100)
}

/// `2026-05-01 12:30:00Z`.
pub fn timestamp(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d %H:%M:%SZ").to_string()
}

/// `1.5 MiB`.
#[expect(
    clippy::cast_precision_loss,
    reason = "sizes are shown with one decimal"
)]
pub fn bytes(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `2h 05m`, `12m`, `45s`.
pub fn seconds(total: i64) -> String {
    let total = total.max(0);
    let (hours, minutes) = (total / 3600, total % 3600 / 60);
    match (hours, minutes) {
        (0, 0) => format!("{total}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m:02}m"),
    }
}

/// `12.3%`.
pub fn percent(value: f64) -> String {
    format!("{value:.1}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_rounds_to_cents_and_keeps_the_sign() {
        assert_eq!(money(12_345_678, Currency::Usd), "12.35 USD");
        assert_eq!(money(-1_500_000, Currency::Eur), "-1.50 EUR");
        assert_eq!(money(-1, Currency::Usd), "0.00 USD");
    }

    #[test]
    fn sizes_and_durations_are_short() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1_572_864), "1.5 MiB");
        assert_eq!(seconds(7_500), "2h 05m");
        assert_eq!(seconds(90), "1m");
        assert_eq!(seconds(5), "5s");
    }
}
