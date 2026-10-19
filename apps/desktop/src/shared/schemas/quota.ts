import { z } from "zod";
import { countSchema, currencySchema, microsSchema, timestampSchema } from "./primitives";
import { planSchema } from "./usage";

/** Forecast of when a window runs out (`QuotaEta`, tagged by `kind`). */
const quotaEtaSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("will_hit"), at: timestampSchema }),
  z.object({ kind: z.literal("wont_hit") }),
  z.object({ kind: z.literal("not_enough_data") }),
]);
export type QuotaEta = z.infer<typeof quotaEtaSchema>;

/** Spend of a money window (`QuotaSpend`), converted to the display currency. */
const quotaSpendSchema = z.object({
  currency: currencySchema,
  spent_micros: microsSchema,
  limit_micros: microsSchema,
  /** Converted with an old FX rate. */
  approximate: z.boolean(),
});
export type QuotaSpend = z.infer<typeof quotaSpendSchema>;

/** Claude's monthly pay-as-you-go window; the only one with `spend`, and no reset time. */
export const EXTRA_USAGE_WINDOW = "extra_usage";

const quotaWindowSchema = z.object({
  /** `five_hour`, `seven_day`, `weekly_scoped:<model>`, ..., `extra_usage` (last). */
  window: z.string(),
  /** Display name from the backend, e.g. `Weekly · Fable`. */
  label: z.string(),
  /** May exceed 100. */
  used_percent: z.number(),
  resets_at: timestampSchema.nullable(),
  resets_in_secs: z.number().int().nullable(),
  eta: quotaEtaSchema,
  /** Spent and cap this month; set on `extra_usage` only. */
  spend: quotaSpendSchema.nullable(),
});
export type QuotaWindow = z.infer<typeof quotaWindowSchema>;

const QUOTA_STATUSES = ["ok", "signed_out", "token_expired", "failed", "keychain_denied"] as const;
const quotaStatusSchema = z.enum(QUOTA_STATUSES);
export type QuotaStatus = z.infer<typeof quotaStatusSchema>;

/** The latest poll of one agent profile. `windows` is empty unless `status` is `ok`. */
export const quotaMeterSchema = z.object({
  agent: z.string(),
  profile: z.string(),
  fetched_at: timestampSchema,
  age_secs: z.number().int(),
  status: quotaStatusSchema,
  account: z.string().nullable(),
  windows: z.array(quotaWindowSchema),
});
export type QuotaMeter = z.infer<typeof quotaMeterSchema>;

const quotaPointSchema = z.object({ at: timestampSchema, used_percent: z.number() });

export const quotaHistorySchema = z.object({
  agent: z.string(),
  profile: z.string(),
  window: z.string(),
  /** Display name from the backend. */
  label: z.string(),
  points: z.array(quotaPointSchema),
  /** Distinct reset times inside the range. */
  resets: z.array(timestampSchema),
});
export type QuotaHistory = z.infer<typeof quotaHistorySchema>;

const verdictSchema = z.discriminatedUnion("verdict", [
  z.object({ verdict: z.literal("keep") }),
  z.object({ verdict: z.literal("downgrade"), plan: planSchema }),
  z.object({ verdict: z.literal("upgrade"), plan: planSchema }),
  z.object({ verdict: z.literal("pay_as_you_go") }),
]);

const planFitSchema = z.object({
  plan: planSchema,
  monthly_fee: microsSchema,
  five_hour_peak: z.number(),
  seven_day_peak: z.number(),
  fits: z.boolean(),
});

/** Where the current plan's quota size came from (`QuotaSizing`, tagged by `source`). */
const quotaSizingSchema = z.discriminatedUnion("source", [
  z.object({ source: z.literal("catalog") }),
  /** Claude's rate-limit tier: the seat has `plan`'s quota. */
  z.object({ source: z.literal("rate_tier"), plan: planSchema }),
]);

/** The advisor's output (`Advice`, tagged by `advice`; `recommend` is flattened). */
const adviceSchema = z.discriminatedUnion("advice", [
  z.object({ advice: z.literal("not_enough_data"), span_days: countSchema }),
  /** No plan is known for the account. */
  z.object({ advice: z.literal("no_plan") }),
  /** The plan has no catalog quota and no rate-limit tier sizes it. */
  z.object({ advice: z.literal("unknown_quota") }),
  z.object({
    advice: z.literal("recommend"),
    verdict: verdictSchema,
    current: planSchema,
    sizing: quotaSizingSchema,
    /** Declared fee, else the list fee; `null` when neither is known. */
    current_monthly_fee: microsSchema.nullable(),
    headroom: countSchema,
    fits: z.array(planFitSchema),
  }),
]);
export type Advice = z.infer<typeof adviceSchema>;

export const advisorReportSchema = z.object({
  agent: z.string(),
  profile: z.string(),
  current_plan: planSchema.nullable(),
  /** Organization of the Claude login, e.g. a Team's name. */
  organization: z.string().nullable(),
  snapshots: countSchema,
  monthly_api_value_usd_micros: microsSchema,
  advice: adviceSchema,
});
export type AdvisorReport = z.infer<typeof advisorReportSchema>;
