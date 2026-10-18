import type { ReactNode } from "react";
import { joinRemotes, toRemote, useDaemonGate } from "../../features/common/use-remote";
import { InsightsScreen } from "../../features/insights/insights-screen";
import { useAppConfig } from "../../shared/app-context";
import { useBranches, useHeatmap, useSignals, useTools } from "../../shared/queries";
import { useFilters } from "../use-filters";

/** Insights: tool calls, signals, cost per branch and the activity heatmap. */
export function InsightsRoute(): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  const gate = useDaemonGate();
  const tools = toRemote(useTools(filters), gate);
  const signals = toRemote(useSignals(filters), gate);
  const heatmap = toRemote(useHeatmap(filters), gate);
  // Branches are scoped to one project; without one the panel is empty rather than an error.
  const branchQuery = useBranches(filters);
  const branches = toRemote(branchQuery, gate);
  const remote = joinRemotes(
    joinRemotes(tools, signals, (tools, signals) => ({ tools, signals })),
    joinRemotes(
      heatmap,
      branches.kind === "error" ? { kind: "ready", data: [] } : branches,
      (heatmap, branches) => ({ heatmap, branches }),
    ),
    (left, right) => ({ ...left, ...right, currency: filters.currency }),
  );
  return <InsightsScreen remote={remote} timeZone={timeZone} />;
}
