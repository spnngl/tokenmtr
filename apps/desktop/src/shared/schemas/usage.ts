import { z } from "zod";
import {
  countSchema,
  currencySchema,
  dateSchema,
  microsSchema,
  timestampSchema,
  usageSchema,
  usageShape,
} from "./primitives";

/** Overview KPIs (`Totals`). Percentages are 0-100. */
export const totalsSchema = z.object({
  currency: currencySchema,
  /** EUR was requested but no FX rate exists: the figures are USD. */
  currency_fallback: z.boolean(),
  ...usageShape,
  cached_input_percent: z.number(),
  cache_savings_micros: microsSchema.nullable(),
  roi: z.number().nullable(),
  sessions: countSchema,
});
export type Totals = z.infer<typeof totalsSchema>;

const BUCKETS = ["day", "week", "month"] as const;
const GROUP_BYS = ["none", "agent", "model", "project"] as const;
const bucketSchema = z.enum(BUCKETS);
const groupBySchema = z.enum(GROUP_BYS);
export type GroupBy = z.infer<typeof groupBySchema>;

const seriesPointSchema = z.object({
  /** First local day of the bucket. */
  start: dateSchema,
  /** Real length of the bucket in minutes (1380 or 1500 for a day with a DST change). */
  minutes: countSchema,
  ...usageShape,
  cumulative_api_value_micros: microsSchema,
  cumulative_paid_micros: microsSchema,
  /** Per group key; `(none)` when ungrouped. */
  groups: z.record(
    z.string(),
    z.object({
      total_tokens: countSchema,
      api_value_micros: microsSchema,
      paid_micros: microsSchema,
    }),
  ),
});
export type SeriesPoint = z.infer<typeof seriesPointSchema>;

/** At most ~400 points: the backend picks the bucket. */
export const seriesSchema = z.object({
  currency: currencySchema,
  bucket: bucketSchema,
  group_by: groupBySchema,
  points: z.array(seriesPointSchema),
});
export type Series = z.infer<typeof seriesSchema>;

/** A breakdown table whose money columns are in `currency`. */
const breakdownOf = <Row extends z.ZodType>(row: Row) =>
  z.object({ currency: currencySchema, rows: z.array(row) });

const modelRowSchema = z.object({
  model: z.string(),
  ...usageShape,
  share_percent: z.number(),
  reported_vs_estimated_percent: z.number().nullable(),
  /** Reported and estimated costs disagree beyond the threshold. */
  divergent: z.boolean(),
});
export type ModelRow = z.infer<typeof modelRowSchema>;
export const modelsSchema = breakdownOf(modelRowSchema);
export type Models = z.infer<typeof modelsSchema>;

const projectRowSchema = z.object({
  project: z.string().nullable(),
  name: z.string().nullable(),
  client: z.string().nullable(),
  ...usageShape,
  share_percent: z.number(),
});
export type ProjectRow = z.infer<typeof projectRowSchema>;
export const projectsSchema = breakdownOf(projectRowSchema);
export type Projects = z.infer<typeof projectsSchema>;

const branchRowSchema = z.object({ branch: z.string().nullable(), ...usageShape });
export type BranchRow = z.infer<typeof branchRowSchema>;
export const branchesSchema = breakdownOf(branchRowSchema);
export type Branches = z.infer<typeof branchesSchema>;

/** A working directory no project rule matched (Projects: "Add rule"). */
export const unresolvedCwdSchema = z.object({
  cwd: z.string(),
  events: countSchema,
  api_value_usd_micros: microsSchema,
  last_at: timestampSchema,
});
export type UnresolvedCwd = z.infer<typeof unresolvedCwdSchema>;

const sessionRowSchema = z.object({
  agent: z.string(),
  session: z.string(),
  profile: z.string(),
  started_at: timestampSchema,
  last_at: timestampSchema,
  duration_secs: countSchema,
  project: z.string().nullable(),
  models: z.array(z.string()),
  tool_calls: countSchema,
  tool_errors: countSchema,
  api_errors: countSchema,
  ...usageShape,
});
export type SessionRow = z.infer<typeof sessionRowSchema>;

export const sessionPageSchema = z.object({
  currency: currencySchema,
  total: countSchema,
  /** 0-based. */
  page: countSchema,
  page_size: countSchema,
  rows: z.array(sessionRowSchema),
});
export type SessionPage = z.infer<typeof sessionPageSchema>;

const COST_SOURCES = ["reported", "estimated", "unpriced"] as const;

/** What produced a usage event (`EventKind` in `tokenmtr-core`). */
const EVENT_KINDS = [
  "message",
  "tool_result",
  "compaction",
  "branch_summary",
  "auxiliary",
  "daily_aggregate",
] as const;
export type EventKind = (typeof EVENT_KINDS)[number];

const sessionEventSchema = z.object({
  at: timestampSchema,
  model: z.string(),
  kind: z.enum(EVENT_KINDS),
  git_branch: z.string().nullable(),
  total_tokens: countSchema,
  output: countSchema,
  api_value_micros: microsSchema,
  cost_source: z.enum(COST_SOURCES),
});
export type SessionEvent = z.infer<typeof sessionEventSchema>;

const sessionToolCallSchema = z.object({
  at: timestampSchema,
  tool_name: z.string(),
  mcp_server: z.string().nullable(),
  is_error: z.boolean().nullable(),
});
export type SessionToolCall = z.infer<typeof sessionToolCallSchema>;

const SIGNAL_KINDS = ["api_error", "compaction"] as const;

export const sessionDetailSchema = z.object({
  currency: currencySchema,
  events: z.array(sessionEventSchema),
  /** More events exist than were returned. */
  events_truncated: z.boolean(),
  tool_calls: z.array(sessionToolCallSchema),
  signals: z.array(z.tuple([timestampSchema, z.enum(SIGNAL_KINDS)])),
});
export type SessionDetail = z.infer<typeof sessionDetailSchema>;

/** `weekday` 1 = Monday .. 7 = Sunday; `hour` 0-23 in the configured zone. */
const heatCellSchema = z.object({
  weekday: z.number().int().min(1).max(7),
  hour: z.number().int().min(0).max(23),
  events: countSchema,
  total_tokens: countSchema,
  api_value_micros: microsSchema,
});
export type HeatCell = z.infer<typeof heatCellSchema>;

export const heatmapSchema = z.object({
  currency: currencySchema,
  cells: z.array(heatCellSchema),
});
export type Heatmap = z.infer<typeof heatmapSchema>;

const toolRowSchema = z.object({
  tool_name: z.string().nullable(),
  mcp_server: z.string().nullable(),
  calls: countSchema,
  errors: countSchema,
  error_rate_percent: z.number(),
  sessions: countSchema,
});
export type ToolRow = z.infer<typeof toolRowSchema>;

export const toolsReportSchema = z.object({
  tools: z.array(toolRowSchema),
  mcp_servers: z.array(toolRowSchema),
});
export type ToolsReport = z.infer<typeof toolsReportSchema>;

const signalDaySchema = z.object({
  date: dateSchema,
  api_errors: countSchema,
  compactions: countSchema,
});

const sessionCompactionsSchema = z.object({
  agent: z.string(),
  session: z.string(),
  compactions: countSchema,
});

export const signalsReportSchema = z.object({
  days: z.array(signalDaySchema),
  api_errors: countSchema,
  compactions: countSchema,
  top_sessions: z.array(sessionCompactionsSchema),
});
export type SignalsReport = z.infer<typeof signalsReportSchema>;

const PLANS = ["pro", "max_5x", "max_20x", "team", "enterprise", "custom"] as const;
export const planSchema = z.enum(PLANS);
export type Plan = z.infer<typeof planSchema>;

const subscriptionLineSchema = z.object({
  id: z.string(),
  provider: z.string(),
  plan: planSchema,
  fee_micros: microsSchema,
  /** Currency the fee is billed in. */
  fee_currency: currencySchema,
  period: z.enum(["monthly", "yearly"]),
  start: dateSchema,
  end: dateSchema.nullable(),
  usage: usageSchema,
  roi: z.number().nullable(),
});
export type SubscriptionLine = z.infer<typeof subscriptionLineSchema>;

const billingMonthSchema = z.object({
  /** First day of the month. */
  month: dateSchema,
  ...usageShape,
  roi: z.number().nullable(),
});

export const billingReportSchema = z.object({
  currency: currencySchema,
  total: usageSchema,
  api: usageSchema,
  subscriptions: z.array(subscriptionLineSchema),
  months: z.array(billingMonthSchema),
  roi: z.number().nullable(),
});
export type BillingReport = z.infer<typeof billingReportSchema>;
