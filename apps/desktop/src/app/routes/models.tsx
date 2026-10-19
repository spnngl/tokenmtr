import type { ReactNode } from "react";
import { ModelsScreen } from "../../features/models/models-screen";
import { useRevealConfig } from "../../shared/actions";
import { useModels, useSeries } from "../../shared/queries";
import { toRemote } from "../../shared/use-remote";
import { useFilters } from "../use-filters";

/** Models: cost over time, shares, token mix and ratios per model, unpriced and divergent warnings. */
export function ModelsRoute(): ReactNode {
  const { filters } = useFilters();
  const revealConfig = useRevealConfig();
  return (
    <ModelsScreen
      remote={toRemote(useModels(filters))}
      series={toRemote(useSeries(filters, "model"))}
      // Price overrides live in the config file: show it where the user adds the entry.
      onAddOverride={() => revealConfig.mutate()}
    />
  );
}
