import { z } from "zod";
import { countSchema, microsSchema, timestampSchema } from "./primitives";
import { planSchema } from "./usage";

/** Forecast of when a window runs out (`QuotaEta`, tagged by `kind`). */
const quotaEtaSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("will_hit"), at: timestampSchema }),
  z.object({ kind: z.literal("wont_hit") }),
  z.object({ kind: z.literal("not_enough_data") }),
]);
export type QuotaEta = z.infer<typeof quotaEtaSchema>;

const quotaWindowSchema = z.object({
  /** `five_hour`, `seven_day`, ... */
  window: z.string(),
  /** May exceed 100. */
  used_percent: z.number(),
  resets_at: timestampSchema.nullable(),
  resets_in_secs: z.number().int().nullable(),
  eta: quotaEtaSchema,
});
export type QuotaWindow = z.infer<typeof quotaWindowSchema>;

const QUOTA_STATUSES = ["ok", "signed_out", "token_expired", "failed"] as const;
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

/** The advisor's output (`Advice`, tagged by `advice`; `recommend` is flattened). */
const adviceSchema = z.discriminatedUnion("advice", [
  z.object({ advice: z.literal("not_enough_data"), span_days: countSchema }),
  z.object({ advice: z.literal("not_applicable") }),
  z.object({
    advice: z.literal("recommend"),
    verdict: verdictSchema,
    current: planSchema,
    headroom: countSchema,
    fits: z.array(planFitSchema),
  }),
]);
export type Advice = z.infer<typeof adviceSchema>;

export const advisorReportSchema = z.object({
  agent: z.string(),
  profile: z.string(),
  current_plan: planSchema.nullable(),
  snapshots: countSchema,
  monthly_api_value_usd_micros: microsSchema,
  advice: adviceSchema,
});
export type AdvisorReport = z.infer<typeof advisorReportSchema>;
