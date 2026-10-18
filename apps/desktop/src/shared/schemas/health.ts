import { z } from "zod";
import { countSchema, dateSchema, timestampSchema } from "./primitives";

const sinkRunSchema = z.object({
  dataset: z.string(),
  started_at: timestampSchema,
  finished_at: timestampSchema,
  rows: countSchema,
  error: z.string().nullable(),
});
export type SinkRun = z.infer<typeof sinkRunSchema>;

/** `SinkState` is flattened into `SinkHealth`, tagged by `state`. */
const sinkStateShape = {
  sink: z.string(),
  lag_rows: countSchema,
  lag_secs: z.number().int().nullable(),
  last_success_at: timestampSchema.nullable(),
  recent_runs: z.array(sinkRunSchema),
} as const;

const sinkHealthSchema = z.discriminatedUnion("state", [
  z.object({ state: z.literal("healthy"), ...sinkStateShape }),
  z.object({ state: z.literal("lagging"), ...sinkStateShape }),
  z.object({
    state: z.literal("failing"),
    since: timestampSchema,
    last_error: z.string(),
    ...sinkStateShape,
  }),
  z.object({ state: z.literal("disabled"), ...sinkStateShape }),
]);
export type SinkHealth = z.infer<typeof sinkHealthSchema>;

export const daemonHealthSchema = z.object({
  state: z.enum(["running", "stale", "never_started"]),
  last_heartbeat: timestampSchema.nullable(),
  age_secs: z.number().int().nullable(),
});

const HEALTH_LEVELS = ["ok", "warning", "error"] as const;
const healthLevelSchema = z.enum(HEALTH_LEVELS);
export type HealthLevel = z.infer<typeof healthLevelSchema>;

/** Worst of daemon and sinks: the menu-bar status dot. */
export const systemHealthSchema = z.object({
  level: healthLevelSchema,
  daemon: daemonHealthSchema,
  sinks: z.array(sinkHealthSchema),
});
export type SystemHealth = z.infer<typeof systemHealthSchema>;

export const ingestStatusSchema = z.object({
  agent: z.string(),
  last_run_at: timestampSchema,
  files_parsed: countSchema,
  lines_read: countSchema,
  events_found: countSchema,
  events_changed: countSchema,
  open_issues: countSchema,
});
export type IngestStatus = z.infer<typeof ingestStatusSchema>;

const PARSE_ISSUE_KINDS = [
  "format_drift",
  "unknown_version",
  "protected_folder",
  "missing_cwd",
  "parse_error",
] as const;

export const parseIssueSchema = z.object({
  agent: z.string(),
  kind: z.enum(PARSE_ISSUE_KINDS),
  subject: z.string(),
  occurrences: countSchema,
  sample: z.string().nullable(),
  first_seen: timestampSchema,
  last_seen: timestampSchema,
  resolved_at: timestampSchema.nullable(),
});
export type ParseIssue = z.infer<typeof parseIssueSchema>;

/** `AlertKind` is flattened into `AlertView`, tagged by `kind`. */
const alertShape = { fired_at: timestampSchema, dedup_key: z.string() } as const;

export const alertSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("quota_threshold"),
    window: z.string(),
    percent: countSchema,
    ...alertShape,
  }),
  z.object({ kind: z.literal("quota_reset"), window: z.string(), ...alertShape }),
  z.object({
    kind: z.literal("budget_threshold"),
    budget: z.string(),
    percent: countSchema,
    ...alertShape,
  }),
  z.object({
    kind: z.literal("sink_failing"),
    sink: z.string(),
    since: timestampSchema,
    ...alertShape,
  }),
  z.object({ kind: z.literal("format_drift"), agent: z.string(), ...alertShape }),
  z.object({ kind: z.literal("backup_failed"), ...alertShape }),
]);
export type Alert = z.infer<typeof alertSchema>;

export const backupSchema = z.object({
  started_at: timestampSchema,
  finished_at: timestampSchema,
  path: z.string().nullable(),
  size_bytes: countSchema.nullable(),
  error: z.string().nullable(),
});
export type Backup = z.infer<typeof backupSchema>;

export const fxRateSchema = z.object({
  date: dateSchema,
  /** EUR per 1 USD x 10^9. */
  eur_per_usd_scaled: z.number().int(),
  source: z.string(),
});
export type FxRateView = z.infer<typeof fxRateSchema>;
