import { type ReactNode, useState } from "react";
import { useAppConfig, useDataSource } from "../shared/app-context";
import { useRecap } from "../shared/queries";
import type { Period } from "../shared/schemas";
import { StateView } from "../shared/state-view";
import { Recap } from "./recap";

/** Loads the recap for the chosen period and wires the footer actions to the data source. */
export function RecapApp(): ReactNode {
  const { timeZone, defaultCurrency, now } = useAppConfig();
  const dataSource = useDataSource();
  const [period, setPeriod] = useState<Period>("today");
  const query = useRecap(period, defaultCurrency);

  if (query.data === undefined) {
    return (
      <div className="w-full max-w-[360px] p-3">
        <StateView
          state={
            query.isError
              ? { kind: "error", message: query.error.message, onRetry: () => void query.refetch() }
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
      onOpenDashboard={(agent) => void dataSource.openDashboard({ route: "/", agent })}
      onOpenSettings={() => void dataSource.openDashboard({ route: "/settings", agent: null })}
      onScanNow={() => void dataSource.scanNow()}
    />
  );
}
