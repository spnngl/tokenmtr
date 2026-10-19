import { type ReactNode, useMemo, useState } from "react";
import { useDataAction, useScanNow } from "../shared/actions";
import { useAppConfig, useDataSource } from "../shared/app-context";
import type { DashboardTarget } from "../shared/data-source";
import { errorText } from "../shared/errors";
import { useRecap, useSettings } from "../shared/queries";
import type { Period } from "../shared/schemas";
import { StateView } from "../shared/state-view";
import { Recap } from "./recap";

/** Loads the recap for the chosen period and wires the header actions to the data source. */
export function RecapApp(): ReactNode {
  const { timeZone, defaultCurrency, now } = useAppConfig();
  const [period, setPeriod] = useState<Period>("today");
  const query = useRecap(period, defaultCurrency);
  const agents = useSettings().data?.agents;
  const agentLabels = useMemo(
    () => new Map((agents ?? []).map((agent) => [agent.agent, agent.label])),
    [agents],
  );
  const ds = useDataSource();
  const scan = useScanNow();
  const open = useDataAction((ds, target: DashboardTarget) => ds.openDashboard(target), {
    failure: "Could not open the dashboard",
    refetch: false,
  });

  if (query.data === undefined) {
    return (
      <div className="w-full max-w-[360px] p-3">
        <StateView
          state={
            query.isError
              ? { kind: "error", ...errorText(query.error), onRetry: () => void query.refetch() }
              : { kind: "loading" }
          }
        />
      </div>
    );
  }
  return (
    <Recap
      snapshot={query.data}
      period={period}
      timeZone={timeZone}
      now={now()}
      onPeriodChange={setPeriod}
      onOpenDashboard={(agent) => open.mutate({ route: "/", agent })}
      onOpenSettings={() => open.mutate({ route: "/settings", agent: null })}
      onScanNow={() => scan.mutate()}
      isScanning={scan.isPending}
      agentLabels={agentLabels}
      // A failed resize leaves the popover at its size, where the recap scrolls: nothing to show.
      onContentHeight={(height) => void ds.fitRecap(height).catch(() => undefined)}
    />
  );
}
