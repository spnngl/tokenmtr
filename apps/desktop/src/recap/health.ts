import type { HealthLevel, SystemHealth } from "../shared/schemas";

export interface HealthSummary {
  readonly level: HealthLevel;
  /** The worst problem in words; "Healthy" when there is none. */
  readonly label: string;
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/** Header status: the worst of daemon and sinks, named so the dot is never alone. */
export function summarizeHealth(system: SystemHealth): HealthSummary {
  const { level, daemon, sinks } = system;
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
  return { level, label: level === "ok" ? "Healthy" : "Degraded" };
}
