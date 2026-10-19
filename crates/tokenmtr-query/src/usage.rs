//! The measures every aggregate shares.

use serde::Serialize;

use crate::engine::{Idle, Mode, Row};

/// Tokens, money and data quality of a set of events.
///
/// Money is in the result's `currency` (integer micros); `api_value_usd_micros` is always USD.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    /// Number of usage events.
    pub events: u64,
    /// Input tokens that were not served from cache.
    pub input_uncached: u64,
    /// Input tokens read from cache.
    pub cache_read: u64,
    /// Tokens written to the 5-minute cache.
    pub cache_write_5m: u64,
    /// Tokens written to the 1-hour cache.
    pub cache_write_1h: u64,
    /// Output tokens (reasoning tokens are a subset, never added).
    pub output: u64,
    /// Reasoning tokens, informational.
    pub reasoning: u64,
    /// Processed tokens: uncached input + cache read + cache writes + output.
    pub total_tokens: u64,
    /// What the usage would cost at API prices: reported cost, else estimate.
    pub api_value_micros: i64,
    /// The same API value in USD.
    pub api_value_usd_micros: i64,
    /// What was actually paid: API cost of pay-as-you-go events, the share of prorated
    /// subscription fees allocated by API-value share, plus Claude extra usage.
    pub paid_micros: i64,
    /// Claude extra usage (pay-as-you-go spend beyond the plan) inside `paid_micros`. It has no
    /// tokens, no API value and is not counted in `events`.
    pub extra_usage_micros: i64,
    /// API value of events covered by a subscription.
    pub subscription_value_micros: i64,
    /// Subscription fees allocated to these events.
    pub subscription_fee_micros: i64,
    /// Events priced by the agent's own reported cost.
    pub reported_events: u64,
    /// Events priced by our estimate.
    pub estimated_events: u64,
    /// Events without any price.
    pub unpriced_events: u64,
    /// True when any estimate, missing price, approximate event or old FX rate contributes.
    pub approximate: bool,
}

impl Usage {
    /// Adds one engine row.
    pub(crate) fn add(&mut self, row: &Row) {
        if row.mode == Mode::Extra {
            self.paid_micros += row.paid;
            self.extra_usage_micros += row.paid;
            self.approximate |= row.approximate;
            return;
        }
        let c = &row.cell;
        self.events += c.events;
        self.input_uncached += c.input_uncached;
        self.cache_read += c.cache_read;
        self.cache_write_5m += c.cache_write_5m;
        self.cache_write_1h += c.cache_write_1h;
        self.output += c.output;
        self.reasoning += c.reasoning;
        self.total_tokens += c.total_tokens();
        self.api_value_micros += row.api;
        self.api_value_usd_micros += c.cost_usd;
        self.paid_micros += row.paid;
        self.reported_events += c.reported;
        self.estimated_events += c.estimated;
        self.unpriced_events += c.unpriced;
        self.approximate |= row.approximate;
        if row.is_subscription() {
            self.subscription_value_micros += row.api;
            self.subscription_fee_micros += row.fee;
        }
    }

    /// Adds a subscription fee that no event absorbed (an idle day).
    pub(crate) fn add_idle(&mut self, idle: &Idle) {
        self.paid_micros += idle.paid;
        self.subscription_fee_micros += idle.paid;
        self.approximate |= idle.approximate;
    }

    /// API value of subscription-covered usage over the fees allocated to it; absent when
    /// no fee applies.
    #[must_use]
    pub fn roi(&self) -> Option<f64> {
        (self.subscription_fee_micros > 0).then(|| {
            ratio(
                self.subscription_value_micros.max(0).unsigned_abs(),
                self.subscription_fee_micros.unsigned_abs(),
            )
        })
    }

    /// Cached share of all input tokens, in percent (0 when there was no input).
    #[must_use]
    pub fn cached_input_percent(&self) -> f64 {
        let input =
            self.input_uncached + self.cache_read + self.cache_write_5m + self.cache_write_1h;
        if input == 0 {
            0.0
        } else {
            100.0 * ratio(self.cache_read, input)
        }
    }
}

/// `a / b` as a float; token counts stay far below 2^53 in practice.
#[expect(
    clippy::cast_precision_loss,
    reason = "display ratio, counts below 2^53"
)]
pub(crate) fn ratio(a: u64, b: u64) -> f64 {
    a as f64 / b as f64
}
