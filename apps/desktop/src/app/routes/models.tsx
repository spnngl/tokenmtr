import type { ReactNode } from "react";
import { joinRemotes, toRemote, useDaemonGate } from "../../features/common/use-remote";
import { ModelsScreen } from "../../features/models/models-screen";
import { useDataSource } from "../../shared/app-context";
import { useModels, useSettings } from "../../shared/queries";
import { useFilters } from "../use-filters";

/** Models: tokens, cost and share per model, with unpriced and divergent warnings. */
export function ModelsRoute(): ReactNode {
  const { filters } = useFilters();
  const dataSource = useDataSource();
  const settings = useSettings();
  const gate = useDaemonGate();
  const rows = toRemote(useModels(filters), gate);
  const remote = joinRemotes(rows, { kind: "ready", data: filters.currency }, (rows, currency) => ({
    currency,
    rows,
  }));
  return (
    <ModelsScreen
      remote={remote}
      // Price overrides live in the config file: show it where the user adds the entry.
      onAddOverride={() => {
        if (settings.data !== undefined) {
          void dataSource.revealPath(settings.data.config_path);
        }
      }}
    />
  );
}
