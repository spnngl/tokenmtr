import { describe, expect, it } from "vitest";
import { SOURCE_FAILING } from "../features/export/fixtures";
import type { SinkHealth, SystemHealth } from "../shared/schemas";
import { summarizeHealth } from "./health";

const RUNNING = { state: "running", last_heartbeat: null, age_secs: null } as const;
const BASE = { sink: "csv", lag_rows: 0, lag_secs: 0, last_success_at: null, recent_runs: [] };
const HEALTHY_SINK: SinkHealth = { ...BASE, state: "healthy" };
const FAILING_SINK: SinkHealth = {
  ...BASE,
  state: "failing",
  since: "2026-06-15T09:00:00Z",
  last_error: "x",
};

describe("summarizeHealth", () => {
  it("is healthy when the daemon runs and no sink complains", () => {
    const system: SystemHealth = {
      level: "ok",
      daemon: RUNNING,
      sinks: [HEALTHY_SINK],
      metrics: [],
      sources: [],
    };
    expect(summarizeHealth(system)).toEqual({ level: "ok", label: "Healthy" });
  });

  it("names a stopped daemon before anything else", () => {
    const system: SystemHealth = {
      level: "error",
      daemon: { state: "stale", last_heartbeat: null, age_secs: 900 },
      sinks: [FAILING_SINK],
      metrics: [],
      sources: [],
    };
    expect(summarizeHealth(system).label).toBe("Daemon offline");
  });

  it("counts failing sinks", () => {
    const system: SystemHealth = {
      level: "error",
      daemon: RUNNING,
      sinks: [FAILING_SINK, { ...FAILING_SINK, sink: "sheet" }],
      metrics: [],
      sources: [],
    };
    expect(summarizeHealth(system).label).toBe("2 sinks failing");
  });

  it("names failing metrics outputs after sinks", () => {
    const system: SystemHealth = {
      level: "warning",
      daemon: RUNNING,
      sinks: [HEALTHY_SINK],
      metrics: [
        {
          state: "failing",
          since: "2026-06-15T09:00:00Z",
          last_error: "http status 429",
          output: "grafana",
          kind: "remote_write",
          last_success_at: null,
          recent_runs: [],
        },
      ],
      sources: [],
    };
    expect(summarizeHealth(system)).toEqual({
      level: "warning",
      label: "1 metrics output failing",
    });
  });

  it("names failing sources after metrics outputs", () => {
    const system: SystemHealth = {
      level: "warning",
      daemon: RUNNING,
      sinks: [HEALTHY_SINK],
      metrics: [],
      sources: [SOURCE_FAILING],
    };
    expect(summarizeHealth(system)).toEqual({ level: "warning", label: "1 source failing" });
  });

  it("ignores a Superset source: only imports raise the header", () => {
    const system: SystemHealth = {
      level: "ok",
      daemon: RUNNING,
      sinks: [],
      metrics: [],
      sources: [{ ...SOURCE_FAILING, kind: "superset" }],
    };
    expect(summarizeHealth(system)).toEqual({ level: "ok", label: "Healthy" });
  });

  it("reports lag as a warning", () => {
    const system: SystemHealth = {
      level: "warning",
      daemon: RUNNING,
      sinks: [{ ...BASE, state: "lagging" }],
      metrics: [],
      sources: [],
    };
    expect(summarizeHealth(system)).toEqual({ level: "warning", label: "1 sink lagging" });
  });
});
