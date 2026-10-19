import { z } from "zod";
import {
  countSchema,
  currencySchema,
  dateSchema,
  microsSchema,
  timestampSchema,
} from "./primitives";
import { SIGNAL_KINDS } from "./usage";

const GRANULARITIES = ["hourly", "daily", "monthly"] as const;
export const granularitySchema = z.enum(GRANULARITIES);
export type Granularity = z.infer<typeof granularitySchema>;

/** `paid_micros` = `subscription_fee_micros` + `extra_usage_micros` + `pay_as_you_go_micros`. */
const billingAmountsShape = {
  /** Cost at API prices (extra usage has none). */
  api_value_micros: microsSchema,
  paid_micros: microsSchema,
  /** Prorated subscription fees; an hour gets an even share of its local day's fee. */
  subscription_fee_micros: microsSchema,
  /** Claude extra usage: paid, never API value. */
  extra_usage_micros: microsSchema,
  /** API cost of events outside any subscription. */
  pay_as_you_go_micros: microsSchema,
} as const;
const billingAmountsSchema = z.object(billingAmountsShape);
export type BillingAmounts = z.infer<typeof billingAmountsSchema>;

const billingPointSchema = z.object({
  /** Start of the bucket (instant). */
  start: timestampSchema,
  /** The same start on the configured zone's clock, `YYYY-MM-DDTHH:MM:SS`. */
  local: z.iso.datetime({ local: true }),
  /** Real length in minutes (DST days are 1380 or 1500). */
  minutes: countSchema,
  ...billingAmountsShape,
  /** `api_value_micros - paid_micros`; negative when paying more than the API value. */
  savings_micros: microsSchema,
  /** `api_value / paid`; null when nothing was paid. */
  roi: z.number().nullable(),
  cumulative_api_value_micros: microsSchema,
  cumulative_paid_micros: microsSchema,
  /** Per agent id when split; idle subscription fees are in the bucket only. */
  agents: z.record(z.string(), billingAmountsSchema),
  approximate: z.boolean(),
});
export type BillingPoint = z.infer<typeof billingPointSchema>;

/** Contiguous, zero-filled buckets of paid versus API value. */
export const billingSeriesSchema = z.object({
  currency: currencySchema,
  granularity: granularitySchema,
  points: z.array(billingPointSchema),
});
export type BillingSeries = z.infer<typeof billingSeriesSchema>;

/** `TokenCounts`: disjoint token columns; `reasoning` is a subset of `output`. */
const tokenMixSchema = z.object({
  input_uncached: countSchema,
  cache_read: countSchema,
  cache_write_5m: countSchema,
  cache_write_1h: countSchema,
  output: countSchema,
  reasoning: countSchema,
});
export type TokenMix = z.infer<typeof tokenMixSchema>;

const sessionPointSchema = z.object({
  agent: z.string(),
  session: z.string(),
  started_at: timestampSchema,
  duration_secs: countSchema,
  api_value_micros: microsSchema,
  /** Pay-as-you-go cost only: subscription fees are not split per session. */
  paid_micros: microsSchema,
  /** 0-100; null without input. */
  cache_hit_percent: z.number().nullable(),
  tokens: tokenMixSchema,
  tool_calls: countSchema,
  tool_errors: countSchema,
});
export type SessionPoint = z.infer<typeof sessionPointSchema>;

/** Sessions with API value in `[min_micros, max_micros)`; the last bucket has no upper edge. */
const costBucketSchema = z.object({
  min_micros: microsSchema,
  max_micros: microsSchema.nullable(),
  sessions: countSchema,
});
export type CostBucket = z.infer<typeof costBucketSchema>;

export const sessionAnalyticsSchema = z.object({
  currency: currencySchema,
  /** Every session in range; `rows` holds at most 500 of them. */
  total: countSchema,
  /** Highest API value first. */
  rows: z.array(sessionPointSchema),
  /** Over every session: 0, then 1-2-5 steps from 0.01 to 100 in the result currency. */
  histogram: z.array(costBucketSchema),
  /** Sessions started per local day, zero-filled. */
  days: z.array(z.object({ day: dateSchema, sessions: countSchema })),
  /** 168 cells, weekday-major; `weekday` 1 = Monday .. 7 = Sunday, local `hour` 0-23. */
  starts: z.array(
    z.object({
      weekday: z.number().int().min(1).max(7),
      hour: z.number().int().min(0).max(23),
      sessions: countSchema,
    }),
  ),
});
export type SessionAnalytics = z.infer<typeof sessionAnalyticsSchema>;

const timelineEventSchema = z.object({
  at: timestampSchema,
  model: z.string(),
  tokens: tokenMixSchema,
  api_value_micros: microsSchema,
  /** 0 when a subscription covers the event. */
  paid_micros: microsSchema,
  cumulative_api_value_micros: microsSchema,
  cumulative_paid_micros: microsSchema,
});
export type TimelineEvent = z.infer<typeof timelineEventSchema>;

export const sessionTimelineSchema = z.object({
  currency: currencySchema,
  /** At most 5000, in time order. */
  events: z.array(timelineEventSchema),
  events_truncated: z.boolean(),
  signals: z.array(z.object({ at: timestampSchema, kind: z.enum(SIGNAL_KINDS) })),
  /** Most called first. */
  tools: z.array(
    z.object({
      tool_name: z.string(),
      mcp_server: z.string().nullable(),
      calls: countSchema,
      failed: countSchema,
    }),
  ),
});
export type SessionTimeline = z.infer<typeof sessionTimelineSchema>;
