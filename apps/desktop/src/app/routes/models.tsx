import type { ReactNode } from "react";
import { ModelsScreen } from "../../features/models/models-screen";
import { useRevealConfig } from "../../shared/actions";
import { useModels, useOverview } from "../../shared/queries";
import { joinRemotes, toRemote } from "../../shared/use-remote";
import { useFilters } from "../use-filters";

/** Models: tokens, cost and share per model, with unpriced and divergent warnings. */
export function ModelsRoute(): ReactNode {
  const { filters } = useFilters();
  const revealConfig = useRevealConfig();
  const rows = toRemote(useModels(filters));
  // Model rows carry no currency: the totals of the same filters say which one the backend
  // answered in (EUR falls back to USD without a rate, D27).
  const totals = toRemote(useOverview(filters));
  const remote = joinRemotes(rows, totals, (rows, totals) => ({ currency: totals.currency, rows }));
  return (
    <ModelsScreen
      remote={remote}
      // Price overrides live in the config file: show it where the user adds the entry.
      onAddOverride={() => revealConfig.mutate()}
    />
  );
}
