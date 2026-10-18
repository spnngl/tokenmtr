import type { ReactNode } from "react";
import { useFilters } from "../../app/use-filters";
import { useAppConfig } from "../../shared/app-context";
import { QuotaHistoryChart } from "../../shared/charts";
import { formatDateTime, PLACEHOLDER } from "../../shared/format";
import { useQuotaHistory, useQuotaLatest } from "../../shared/queries";
import { QuotaMeters, windowLabel } from "../../shared/quota-meters";
import { CardRemote, RemoteView } from "../../shared/remote-view";
import type { QuotaHistory } from "../../shared/schemas";
import { Card, ChartFrame, type Column, DataTable, PageHeader } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";

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
        return value === null ? PLACEHOLDER : `${value}%`;
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

/** Hidden when the agent has no history in range; a failed read still shows, with Retry. */
function AgentHistory({ agent }: { readonly agent: string }): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  return (
    <CardRemote title={`${agent} history`} remote={toRemote(useQuotaHistory(filters, agent))}>
      {(histories) =>
        histories.map((history) => (
          <ChartFrame
            key={`${history.profile}:${history.window}`}
            title={`${agent} / ${history.profile}: ${windowLabel(history.window)}`}
            description={`Utilization history: ${history.points.length} points.`}
            points={history.points.length}
            table={<HistoryTable histories={[history]} />}
          >
            <div className="h-56">
              <QuotaHistoryChart history={history} timeZone={timeZone} />
            </div>
          </ChartFrame>
        ))
      }
    </CardRemote>
  );
}

/** Quota: latest meters with ETAs, then utilization history per window. */
export function QuotaScreen(): ReactNode {
  const { filters } = useFilters();
  const { timeZone, now } = useAppConfig();
  return (
    <>
      <PageHeader to="/quota" />
      <RemoteView
        remote={toRemote(useQuotaLatest(filters))}
        emptyWhen={(meters) =>
          meters.length === 0
            ? {
                title: "No quota data",
                hint: "Quota appears for agents that report it once a signed-in account has been polled. Agents without quota support are not an error.",
              }
            : null
        }
      >
        {(meters) => (
          <>
            <Card title="Current windows">
              <QuotaMeters meters={meters} timeZone={timeZone} now={now()} />
            </Card>
            {[...new Set(meters.filter((meter) => meter.status === "ok").map((m) => m.agent))].map(
              (agent) => (
                <AgentHistory key={agent} agent={agent} />
              ),
            )}
          </>
        )}
      </RemoteView>
    </>
  );
}
