import { formatClock, formatDay } from "../../shared/format";
import type { SinkHealth, SinkRun } from "../../shared/schemas";

/** The datasets a sink can export (`tokenmtr-export`), with what each one holds. */
export const DATASETS = [
  { id: "usage_events", description: "One row per model call: tokens, cost, project, branch." },
  { id: "tool_calls", description: "One row per tool call, with MCP server and error flag." },
  { id: "subscriptions", description: "Declared subscriptions and their fees." },
  { id: "quota_snapshots", description: "Quota utilization over time." },
  { id: "fx_rates", description: "Exchange rates used to convert EUR." },
] as const;

/** The newest run of a dataset on one sink, as the dataset list shows it. */
export interface DatasetStatus {
  readonly id: string;
  readonly description: string;
  /** `null` when no run of this dataset is recorded yet. */
  readonly lastRun: SinkRun | null;
}

/** Latest recorded run per dataset over all sinks (`recent_runs` are newest first). */
export function datasetStatuses(sinks: readonly SinkHealth[]): readonly DatasetStatus[] {
  return DATASETS.map((dataset) => {
    let lastRun: SinkRun | null = null;
    for (const sink of sinks) {
      for (const run of sink.recent_runs) {
        if (
          run.dataset === dataset.id &&
          (lastRun === null || run.finished_at > lastRun.finished_at)
        ) {
          lastRun = run;
        }
      }
    }
    return { ...dataset, lastRun };
  });
}

const PROTECTED_ERROR = /permission|not permitted|eperm|eacces|operation not allowed|denied/i;

/**
 * Whether a failing sink's error looks like macOS refusing a protected folder (D50): the screen
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

/** True when a sink is in a state worth an "Export now" or "Retry". */
export function canExport(sink: SinkHealth): boolean {
  return sink.state !== "disabled";
}
