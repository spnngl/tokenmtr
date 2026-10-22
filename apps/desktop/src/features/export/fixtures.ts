import type {
  Dataset,
  SinkHealth,
  SinkRun,
  SourceHealth,
  SystemHealth,
} from "../../shared/schemas";

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
  metrics: [],
  sources: [],
};

const SOURCE_BASE = { machines: 1, files: 1, tool_calls: 0 } as const;

export const SOURCE_SUPERSET: SourceHealth = {
  ...SOURCE_BASE,
  state: "healthy",
  source: "superset",
  kind: "superset",
  files: 0,
  machines: 0,
  events: 0,
  last_success_at: null,
  recent_runs: [],
};

export const SOURCE_HEALTHY: SourceHealth = {
  ...SOURCE_BASE,
  state: "healthy",
  source: "laptop",
  kind: "sqlite",
  events: 8_420,
  tool_calls: 1_310,
  last_success_at: "2026-05-21T12:00:00Z",
  recent_runs: [
    {
      path: null,
      started_at: "2026-05-21T12:00:00Z",
      finished_at: "2026-05-21T12:00:02Z",
      rows: 40,
      error: null,
    },
  ],
};

export const SOURCE_FAILING: SourceHealth = {
  ...SOURCE_BASE,
  state: "failing",
  since: "2026-05-21T10:00:00Z",
  last_error: "database is locked",
  source: "workstation",
  kind: "sqlite",
  events: 3_050,
  tool_calls: 402,
  last_success_at: "2026-05-21T10:00:00Z",
  recent_runs: [
    {
      path: null,
      started_at: "2026-05-21T12:00:00Z",
      finished_at: "2026-05-21T12:00:01Z",
      rows: 0,
      error: "database is locked",
    },
  ],
};

export const SOURCE_DISABLED: SourceHealth = {
  ...SOURCE_BASE,
  state: "disabled",
  source: "old-desktop",
  kind: "sqlite",
  events: 910,
  tool_calls: 88,
  last_success_at: "2026-04-21T12:00:00Z",
  recent_runs: [],
};

/** [`EXPORT_HEALTH`] plus every source state. */
export const EXPORT_HEALTH_WITH_SOURCES: SystemHealth = {
  ...EXPORT_HEALTH,
  sources: [SOURCE_SUPERSET, SOURCE_HEALTHY, SOURCE_FAILING, SOURCE_DISABLED],
};
