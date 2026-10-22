import { formatClock, formatDay } from "../../shared/format";
import {
  type Dataset,
  datasetSchema,
  type SinkHealth,
  type SinkRun,
  type SourceHealth,
  type SystemHealth,
} from "../../shared/schemas";

/** What each exported dataset (`tokenmtr-export`) holds. */
const DESCRIPTIONS: Readonly<Record<Dataset, string>> = {
  usage_events: "One row per model call: tokens, cost, project, branch.",
  tool_calls: "One row per tool call, with MCP server and error flag.",
  subscriptions: "Declared subscriptions and their fees.",
  quota_snapshots: "Quota utilization over time.",
  fx_rates: "Exchange rates used to convert EUR.",
};

/** The newest run of a dataset on one sink, as the dataset list shows it. */
export interface DatasetStatus {
  readonly id: Dataset;
  readonly description: string;
  /** `null` when no run of this dataset is recorded yet. */
  readonly lastRun: SinkRun | null;
}

/** Latest recorded run per dataset over all sinks (`recent_runs` are newest first). */
export function datasetStatuses(sinks: readonly SinkHealth[]): readonly DatasetStatus[] {
  return datasetSchema.options.map((id) => {
    let lastRun: SinkRun | null = null;
    for (const sink of sinks) {
      for (const run of sink.recent_runs) {
        if (run.dataset === id && (lastRun === null || run.finished_at > lastRun.finished_at)) {
          lastRun = run;
        }
      }
    }
    return { id, description: DESCRIPTIONS[id], lastRun };
  });
}

const PROTECTED_ERROR = /permission|not permitted|eperm|eacces|operation not allowed|denied/i;

/**
 * Whether a failing sink's error looks like macOS refusing a protected folder: the screen
 * then explains how to grant access instead of only echoing the error.
 */
export function looksLikeProtectedFolder(sink: SinkHealth): boolean {
  switch (sink.state) {
    case "failing":
      return PROTECTED_ERROR.test(sink.last_error);
    case "healthy":
    case "lagging":
    case "disabled":
      return false;
    default:
      return sink satisfies never;
  }
}

/** `2026-05-21 14:03` in the configured zone. */
export function formatInstant(timestamp: string, timeZone: string): string {
  const ms = Date.parse(timestamp);
  return `${formatDay(ms, timeZone)} ${formatClock(ms, timeZone)}`;
}

/** The sources that hold imported rows; a `superset` source only names projects and has no card. */
export function sqliteSources(health: SystemHealth): readonly SourceHealth[] {
  return health.sources.filter((source) => source.kind === "sqlite");
}

/** True when a sink is in a state worth an "Export now" or "Retry". */
export function canExport(sink: SinkHealth): boolean {
  return sink.state !== "disabled";
}
