import type { ReactNode } from "react";
import { type Remote, RemoteView } from "../../shared/remote-view";
import { PageHeader } from "../../shared/ui";
import { BranchesPanel } from "./branches-panel";
import { HeatmapPanel } from "./heatmap-panel";
import { type InsightsData, isInsightsEmpty } from "./model";
import { SignalsPanel } from "./signals-panel";
import { ToolsPanel } from "./tools-panel";

interface InsightsScreenProps {
  readonly remote: Remote<InsightsData>;
  /** IANA zone the heatmap hours are in. */
  readonly timeZone: string;
}

/** The Insights page: tools, signals, branches and the activity heatmap, in every view state. */
export function InsightsScreen({ remote, timeZone }: InsightsScreenProps): ReactNode {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader to="/insights" />
      <RemoteView
        remote={remote}
        emptyWhen={(data) =>
          isInsightsEmpty(data)
            ? {
                title: "No activity to analyse yet",
                hint: "Use an agent, then run a scan: tool calls, errors and branches show up here.",
              }
            : null
        }
      >
        {(data) => (
          <div className="flex flex-col gap-8">
            <ToolsPanel report={data.tools} />
            <SignalsPanel report={data.signals} />
            <BranchesPanel rows={data.branches.rows} currency={data.branches.currency} />
            <HeatmapPanel heatmap={data.heatmap} timeZone={timeZone} />
          </div>
        )}
      </RemoteView>
    </div>
  );
}
