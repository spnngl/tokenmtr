import type { ReactNode } from "react";
import { InsightsScreen } from "../../features/insights/insights-screen";
import { useAppConfig } from "../../shared/app-context";
import { useBranches, useHeatmap, useSeries, useSignals, useTools } from "../../shared/queries";
import { joinRemotes, toRemote } from "../../shared/use-remote";
import { useFilters } from "../use-filters";

/** Insights: tool calls, signals, cost per model and branch, and the activity heatmap. */
export function InsightsRoute(): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  const tools = toRemote(useTools(filters));
  const signals = toRemote(useSignals(filters));
  const heatmap = toRemote(useHeatmap(filters));
  const branches = toRemote(useBranches(filters));
  const costs = toRemote(useSeries(filters, "model"));
  const remote = joinRemotes(
    joinRemotes(
      joinRemotes(tools, signals, (tools, signals) => ({ tools, signals })),
      joinRemotes(heatmap, branches, (heatmap, branches) => ({ heatmap, branches })),
      (left, right) => ({ ...left, ...right }),
    ),
    costs,
    (rest, costs) => ({ ...rest, costs }),
  );
  return <InsightsScreen remote={remote} timeZone={timeZone} />;
}
