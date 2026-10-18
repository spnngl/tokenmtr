import { z } from "zod";
import { systemHealthSchema } from "./health";
import {
  countSchema,
  currencySchema,
  dateSchema,
  microsSchema,
  timestampSchema,
} from "./primitives";
import { quotaMeterSchema } from "./quota";
import { totalsSchema } from "./usage";

export const PERIODS = ["today", "week", "month"] as const;
const periodSchema = z.enum(PERIODS);
export type Period = z.infer<typeof periodSchema>;

const budgetBase = {
  id: z.string(),
  period: z.enum(["day", "week", "month"]),
  measure: z.enum(["paid", "api_value"]),
  /** The budget's own currency, whatever the display currency is. */
  currency: currencySchema,
  period_start: dateSchema,
  /** Exclusive. */
  period_end: dateSchema,
  limit_micros: microsSchema,
};

/** `BudgetView` of the query crate, by `state` (`BudgetState`). */
export const budgetSchema = z.discriminatedUnion("state", [
  z.object({
    state: z.literal("measured"),
    ...budgetBase,
    spent_micros: microsSchema,
    /** Whole percent; may exceed 100. */
    percent: countSchema,
    projected_micros: microsSchema,
    projected_percent: countSchema,
  }),
  /**
   * The budget's currency needs an FX rate and the store has none yet: only the limit and period
   * are real. Rust sends zeroes for the rest; the schema drops them so no screen shows them.
   */
  z.object({ state: z.literal("no_fx_rate"), ...budgetBase }),
]);
export type Budget = z.infer<typeof budgetSchema>;
export type MeasuredBudget = Extract<Budget, { state: "measured" }>;

const liveSchema = z.object({
  tokens_per_minute: countSchema,
  api_value_per_hour_usd_micros: microsSchema,
  active_sessions: countSchema,
});
export type Live = z.infer<typeof liveSchema>;

const agentRecapSchema = z.object({
  /** `null` is the `All` tab. */
  agent: z.string().nullable(),
  period: periodSchema,
  /** Local days, `end` exclusive. */
  range: z.object({ start: dateSchema, end: dateSchema }),
  currency: currencySchema,
  currency_fallback: z.boolean(),
  totals: totalsSchema,
  projected_paid_micros: microsSchema,
  /** Empty when the agent cannot report quota. */
  quota: z.array(quotaMeterSchema),
  budgets: z.array(budgetSchema),
  /** API value of the last seven days, today included. */
  sparkline: z.array(z.object({ date: dateSchema, api_value_micros: microsSchema })),
  /** Absent when the window has no events. */
  live: liveSchema.nullable(),
  top_model: z.string().nullable(),
  last_activity: timestampSchema.nullable(),
});
export type AgentRecap = z.infer<typeof agentRecapSchema>;

export const STATUS_SCHEMA_VERSION = 1;

/** `status --json` (D43): system health and one recap per followed agent. */
const statusSchema = z.object({
  status_schema: z.literal(STATUS_SCHEMA_VERSION),
  generated_at: timestampSchema,
  system: systemHealthSchema,
  agents: z.array(agentRecapSchema),
});
export type Status = z.infer<typeof statusSchema>;

/**
 * What the menu-bar recap loads: `status` plus the `All` recap in front of it when more than one
 * agent is followed. The Tauri command composes the two Rust documents (`recap` and `status`).
 */
export const recapSnapshotSchema = z.object({
  status_schema: z.literal(STATUS_SCHEMA_VERSION),
  generated_at: timestampSchema,
  system: systemHealthSchema,
  tabs: z.array(agentRecapSchema),
});
export type RecapSnapshot = z.infer<typeof recapSnapshotSchema>;
