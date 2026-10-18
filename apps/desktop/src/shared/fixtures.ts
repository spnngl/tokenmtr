import type { Usage } from "./schemas/primitives";

const MICROS = 1_000_000;

/** Whole currency units to integer micros. */
export function micros(units: number): number {
  return Math.round(units * MICROS);
}

/** A `Usage` block with `overrides`; totals are consistent with the token fields given. */
export function usage(overrides: Partial<Usage> = {}): Usage {
  const base: Usage = {
    events: 0,
    input_uncached: 0,
    cache_read: 0,
    cache_write_5m: 0,
    cache_write_1h: 0,
    output: 0,
    reasoning: 0,
    total_tokens: 0,
    api_value_micros: 0,
    api_value_usd_micros: 0,
    paid_micros: 0,
    subscription_value_micros: 0,
    subscription_fee_micros: 0,
    reported_events: 0,
    estimated_events: 0,
    unpriced_events: 0,
    approximate: false,
  };
  const merged = { ...base, ...overrides };
  const total =
    overrides.total_tokens ??
    merged.input_uncached +
      merged.cache_read +
      merged.cache_write_5m +
      merged.cache_write_1h +
      merged.output;
  return { ...merged, total_tokens: total };
}
