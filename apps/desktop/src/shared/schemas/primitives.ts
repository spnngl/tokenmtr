import { z } from "zod";
import { CURRENCIES } from "../format";

/**
 * Wire primitives shared by every query result (crate docs of `tokenmtr-query`): snake_case keys,
 * RFC 3339 timestamps, `YYYY-MM-DD` dates, money as integer micro-units in the result's
 * `currency`. `z.number().int()` accepts safe integers only, which is why money stays a `number`.
 */

export const microsSchema = z.number().int();
export const countSchema = z.number().int().nonnegative();
export const timestampSchema = z.iso.datetime({ offset: true });
export const dateSchema = z.iso.date();
export const currencySchema = z.enum(CURRENCIES);

/** `Usage` of the query crate: flattened into every aggregate row. */
export const usageShape = {
  events: countSchema,
  input_uncached: countSchema,
  cache_read: countSchema,
  cache_write_5m: countSchema,
  cache_write_1h: countSchema,
  output: countSchema,
  reasoning: countSchema,
  total_tokens: countSchema,
  /** Cost at API rates, in the result currency. */
  api_value_micros: microsSchema,
  /** Same, always in USD (never converted). */
  api_value_usd_micros: microsSchema,
  /** Money actually paid: API cost, prorated subscription fees and Claude extra usage. */
  paid_micros: microsSchema,
  /** Claude extra usage (pay-as-you-go beyond the plan) inside `paid_micros`; no tokens. */
  extra_usage_micros: microsSchema,
  subscription_value_micros: microsSchema,
  subscription_fee_micros: microsSchema,
  reported_events: countSchema,
  estimated_events: countSchema,
  unpriced_events: countSchema,
  /** An estimate, missing price, approximate event or old FX rate contributes. */
  approximate: z.boolean(),
} as const;

export const usageSchema = z.object(usageShape);
export type Usage = z.infer<typeof usageSchema>;
