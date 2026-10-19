import type { HealthLevel, SystemHealth } from "../shared/schemas";

export interface HealthSummary {
  readonly level: HealthLevel;
  /** The worst problem in words; "Healthy" when there is none. */
  readonly label: string;
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/** Header status: the worst of daemon, sinks and metrics outputs, named so the dot is never alone. */
export function summarizeHealth(system: SystemHealth): HealthSummary {
  const { level, daemon, sinks, metrics } = system;
  if (daemon.state !== "running") {
    return { level, label: "Daemon offline" };
  }
  const failing = sinks.filter((sink) => sink.state === "failing").length;
  if (failing > 0) {
    return { level, label: `${plural(failing, "sink")} failing` };
  }
  const lagging = sinks.filter((sink) => sink.state === "lagging").length;
  if (lagging > 0) {
    return { level, label: `${plural(lagging, "sink")} lagging` };
  }
  const failingOutputs = metrics.filter((output) => output.state === "failing").length;
  if (failingOutputs > 0) {
    return { level, label: `${plural(failingOutputs, "metrics output")} failing` };
  }
  return { level, label: level === "ok" ? "Healthy" : "Degraded" };
}
