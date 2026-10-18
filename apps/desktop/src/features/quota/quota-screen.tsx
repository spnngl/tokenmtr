import type { ReactNode } from "react";
import { useFilters } from "../../app/use-filters";
import { useAppConfig } from "../../shared/app-context";
import { QuotaHistoryChart } from "../../shared/charts";
import { formatDateTime } from "../../shared/format";
import { queryViewState, useQuotaHistory, useQuotaLatest } from "../../shared/queries";
import { QuotaMeters, windowLabel } from "../../shared/quota-meters";
import type { QuotaHistory } from "../../shared/schemas";
import { StateView } from "../../shared/state-view";
import { Card, ChartFrame, type Column, DataTable, PageHeader } from "../../shared/ui";

function peak(history: QuotaHistory): number {
  return Math.max(0, ...history.points.map((point) => point.used_percent));
}

function latest(history: QuotaHistory): number | null {
  return history.points.at(-1)?.used_percent ?? null;
}

function HistoryTable({ histories }: { readonly histories: readonly QuotaHistory[] }): ReactNode {
  const { timeZone } = useAppConfig();
  const columns: Column<QuotaHistory>[] = [
    {
      header: "Window",
      cell: (row) => `${row.profile} / ${windowLabel(row.window)}`,
    },
    { header: "Peak", numeric: true, cell: (row) => `${peak(row)}%`, sortValue: peak },
    {
      header: "Latest",
      numeric: true,
      cell: (row) => {
        const value = latest(row);
        return value === null ? "—" : `${value}%`;
      },
    },
    {
      header: "Resets",
      cell: (row) =>
        row.resets.length === 0
          ? "None in range"
          : row.resets.map((reset) => formatDateTime(Date.parse(reset), timeZone)).join(", "),
    },
  ];
  return (
    <DataTable
      caption="Quota utilization summary"
      rows={histories}
      rowKey={(row) => `${row.profile}:${row.window}`}
      columns={columns}
    />
  );
}

function AgentHistory({ agent }: { readonly agent: string }): ReactNode {
  const { filters } = useFilters();
  const query = useQuotaHistory(filters, agent);
  const histories = query.data ?? [];
  if (histories.length === 0) {
    return null;
  }
  return (
    <>
      {histories.map((history) => (
        <ChartFrame
          key={`${history.profile}:${history.window}`}
          title={`${agent} / ${history.profile}: ${windowLabel(history.window)}`}
          description={`Utilization history: ${history.points.length} points.`}
          points={history.points.length}
          table={<HistoryTable histories={[history]} />}
        >
          <div className="h-56">
            <QuotaHistoryChart history={history} />
          </div>
        </ChartFrame>
      ))}
    </>
  );
}

/** Quota (PLAN §7): latest meters with ETAs, then utilization history per window. */
export function QuotaScreen(): ReactNode {
  const { filters } = useFilters();
  const { timeZone, now } = useAppConfig();
  const query = useQuotaLatest(filters);
  const state = queryViewState(query, (meters) =>
    meters.length === 0
      ? {
          title: "No quota data",
          hint: "Quota appears for agents that report it once a signed-in account has been polled. Agents without quota support are not an error.",
        }
      : null,
  );
  const meters = query.data ?? [];
  const agents = [
    ...new Set(meters.filter((meter) => meter.status === "ok").map((meter) => meter.agent)),
  ];
  return (
    <>
      <PageHeader
        title="Quota"
        summary="Utilization of every quota window over time, with reset markers."
      />
      <StateView state={state}>
        <Card title="Current windows">
          <QuotaMeters meters={meters} timeZone={timeZone} now={now()} />
        </Card>
        {agents.map((agent) => (
          <AgentHistory key={agent} agent={agent} />
        ))}
      </StateView>
    </>
  );
}
