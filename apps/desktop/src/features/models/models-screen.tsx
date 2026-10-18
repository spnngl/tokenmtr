import type { ReactNode } from "react";
import { PageHeader } from "../common/page";
import { type Remote, RemoteView } from "../common/remote-view";
import type { ModelsData } from "./model";
import { ModelsView } from "./models-view";

interface ModelsScreenProps {
  readonly remote: Remote<ModelsData>;
  readonly onAddOverride: () => void;
}

/** The Models page: header plus every §7 state. */
export function ModelsScreen({ remote, onAddOverride }: ModelsScreenProps): ReactNode {
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
        {(report) => <ModelsView report={report} onAddOverride={onAddOverride} />}
      </RemoteView>
    </div>
  );
}
