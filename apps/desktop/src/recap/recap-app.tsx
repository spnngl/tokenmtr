import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { useAppConfig, useDataSource } from "../shared/app-context";
import type { Period } from "../shared/recap-model";
import { StateView } from "../shared/state-view";
import { Recap } from "./recap";

/** Loads the recap for the chosen period and wires the footer actions to the data source. */
export function RecapApp(): ReactNode {
  const { timeZone, defaultCurrency, fx } = useAppConfig();
  const dataSource = useDataSource();
  const [period, setPeriod] = useState<Period>("today");
  const query = useQuery({
    queryKey: ["recap", period],
    queryFn: () => dataSource.recap(period),
    placeholderData: keepPreviousData,
  });

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
      model={query.data}
      currency={defaultCurrency}
      fx={fx}
      timeZone={timeZone}
      onPeriodChange={setPeriod}
      onOpenDashboard={(agent) => void dataSource.openDashboard({ route: "/", agent })}
      onOpenSettings={() => void dataSource.openDashboard({ route: "/settings", agent: null })}
      onScanNow={() => void dataSource.scanNow()}
    />
  );
}
