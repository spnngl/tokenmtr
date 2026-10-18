import type { ReactNode } from "react";
import { InsightsScreen } from "../../features/insights/insights-screen";
import { useAppConfig } from "../../shared/app-context";
import { useBranches, useHeatmap, useSignals, useTools } from "../../shared/queries";
import { joinRemotes, toRemote } from "../../shared/use-remote";
import { useFilters } from "../use-filters";

/** Insights: tool calls, signals, cost per branch and the activity heatmap. */
export function InsightsRoute(): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  const tools = toRemote(useTools(filters));
  const signals = toRemote(useSignals(filters));
  const heatmap = toRemote(useHeatmap(filters));
  const branches = toRemote(useBranches(filters));
  const remote = joinRemotes(
    joinRemotes(tools, signals, (tools, signals) => ({ tools, signals })),
    joinRemotes(heatmap, branches, (heatmap, branches) => ({ heatmap, branches })),
    // Branch rows carry no currency: the heatmap of the same filters says which one the backend
    // answered in (EUR falls back to USD without a rate, D27).
    (left, right) => ({ ...left, ...right, currency: right.heatmap.currency }),
  );
  return <InsightsScreen remote={remote} timeZone={timeZone} />;
}
