import type { ReactNode } from "react";
import { type Remote, RemoteView } from "../../shared/remote-view";
import type { Series } from "../../shared/schemas";
import { PageHeader } from "../../shared/ui";
import type { ModelsData } from "./model";
import { ModelsView } from "./models-view";

interface ModelsScreenProps {
  readonly remote: Remote<ModelsData>;
  /** API value per bucket grouped by model, for the cost-over-time chart. */
  readonly series: Remote<Series>;
  readonly onAddOverride: () => void;
}

/** The Models page: header plus every view state. */
export function ModelsScreen({ remote, series, onAddOverride }: ModelsScreenProps): ReactNode {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader to="/models" />
      <RemoteView
        remote={remote}
        emptyWhen={(report) =>
          report.rows.length === 0
            ? {
                title: "No model usage yet",
                hint: "Use an agent, then run a scan: models appear as soon as events are recorded.",
              }
            : null
        }
      >
        {(report) => <ModelsView report={report} series={series} onAddOverride={onAddOverride} />}
      </RemoteView>
    </div>
  );
}
