import type { Dataset, SinkHealth, SinkRun, SystemHealth } from "../../shared/schemas";

function run(
  dataset: Dataset,
  finished: string,
  rows: number,
  error: string | null = null,
): SinkRun {
  return { dataset, started_at: finished, finished_at: finished, rows, error };
}

export const SINK_HEALTHY: SinkHealth = {
  state: "healthy",
  sink: "local-jsonl",
  lag_rows: 0,
  lag_secs: null,
  last_success_at: "2026-05-21T12:03:00Z",
  recent_runs: [
    run("usage_events", "2026-05-21T12:03:00Z", 120),
    run("tool_calls", "2026-05-21T12:03:00Z", 45),
  ],
};

const SINK_LAGGING: SinkHealth = {
  state: "lagging",
  sink: "warehouse",
  lag_rows: 5400,
  lag_secs: 7200,
  last_success_at: "2026-05-21T09:00:00Z",
  recent_runs: [run("usage_events", "2026-05-21T09:00:00Z", 900)],
};

export const SINK_FAILING: Extract<SinkHealth, { state: "failing" }> = {
  state: "failing",
  sink: "documents-copy",
  since: "2026-05-21T10:00:00Z",
  last_error: "Operation not permitted (os error 1)",
  lag_rows: 300,
  lag_secs: 3600,
  last_success_at: null,
  recent_runs: [
    run("usage_events", "2026-05-21T10:00:00Z", 0, "Operation not permitted (os error 1)"),
  ],
};

export const SINK_DISABLED: SinkHealth = {
  state: "disabled",
  sink: "archive",
  lag_rows: 0,
  lag_secs: null,
  last_success_at: null,
  recent_runs: [],
};

export const EXPORT_HEALTH: SystemHealth = {
  level: "error",
  daemon: { state: "running", last_heartbeat: "2026-05-21T12:04:00Z", age_secs: 20 },
  sinks: [SINK_HEALTHY, SINK_LAGGING, SINK_FAILING, SINK_DISABLED],
};
