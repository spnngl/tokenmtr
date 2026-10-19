import { Link, useNavigate } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { CartesianGrid, Scatter, ScatterChart, Tooltip, XAxis, YAxis } from "recharts";
import { useFilters } from "../../app/use-filters";
import { HorizontalBarChart } from "../../shared/bar-chart";
import { AXIS, chartColor, compactMoney } from "../../shared/charts";
import { ColumnChart, type ColumnDatum } from "../../shared/column-chart";
import { topRows } from "../../shared/decimate";
import {
  type Currency,
  formatCount,
  formatCurrencyMicros,
  formatDuration,
  formatPercent,
  formatTokens,
} from "../../shared/format";
import { useSessionAnalytics } from "../../shared/queries";
import { RankedBars } from "../../shared/ranked-bars";
import { RemoteView } from "../../shared/remote-view";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { SessionAnalytics, SessionPoint } from "../../shared/schemas";
import { Amount, Card, ChartFrame, type Column, DataTable, Section } from "../../shared/ui";
import { useChartAnimation } from "../../shared/use-chart-animation";
import { toRemote } from "../../shared/use-remote";
import { WeekHourTable, weekHourGrid } from "../../shared/week-hour-table";
import {
  cacheHitBins,
  costBucketLabel,
  mixTotal,
  shortSession,
  TOKEN_MIX_SERIES,
  tokenMixRows,
  toolErrorRanking,
} from "./analysis-model";

/** Ranked lists and the token mix show this many sessions. */
const TOP = 10;
/** Scatter point budget (`rows` already holds the costliest sessions first). */
const SCATTER_POINTS = 400;

type OpenSession = (agent: string, session: string) => void;

function SessionLink({
  row,
}: {
  readonly row: Pick<SessionPoint, "agent" | "session">;
}): ReactNode {
  return (
    <Link
      to="/sessions/$agent/$session"
      params={{ agent: row.agent, session: row.session }}
      search={(previous: Record<string, unknown>) => previous}
      className="font-medium text-accent underline"
    >
      {shortSession(row.session)}
      <span className="sr-only"> {row.session.slice(8)}</span>
    </Link>
  );
}

const COUNT_COLUMNS = (header: string): Column<ColumnDatum>[] => [
  { header, cell: (row) => row.label },
  {
    header: "Sessions",
    numeric: true,
    cell: (row) => formatCount(row.value),
    sortValue: (row) => row.value,
  },
];

function CostHistogram({ data }: { readonly data: SessionAnalytics }): ReactNode {
  const columns = data.histogram.map((bucket) => ({
    label: costBucketLabel(bucket, data.currency),
    value: bucket.sessions,
  }));
  return (
    <ChartFrame
      interactive
      title="Sessions by API value"
      description="How many sessions fall in each API value band, over every session in range."
      points={columns.length}
      table={
        <DataTable
          caption="Sessions by API value"
          rows={columns}
          rowKey={(row) => row.label}
          columns={COUNT_COLUMNS("API value")}
        />
      }
    >
      <ColumnChart
        title="Sessions by API value"
        data={columns}
        name="Sessions"
        formatValue={formatCount}
      />
    </ChartFrame>
  );
}

function CostScatter({
  rows,
  currency,
  onOpenSession,
}: {
  readonly rows: readonly SessionPoint[];
  readonly currency: Currency;
  readonly onOpenSession: OpenSession;
}): ReactNode {
  const animate = useChartAnimation();
  const points = topRows(rows, SCATTER_POINTS);
  const data = points.map((row) => ({ duration: row.duration_secs, value: row.api_value_micros }));
  return (
    <ChartFrame
      interactive
      title="API value against duration"
      description="One dot per session: how long it ran and what it would cost at API prices. Select a dot to open the session."
      points={data.length}
      table={
        <DataTable
          caption="API value against duration"
          rows={points}
          rowKey={(row) => `${row.agent}:${row.session}`}
          columns={[
            { header: "Session", cell: (row) => <SessionLink row={row} /> },
            { header: "Agent", cell: (row) => row.agent, sortValue: (row) => row.agent },
            {
              header: "Duration",
              numeric: true,
              cell: (row) => formatDuration(row.duration_secs),
              sortValue: (row) => row.duration_secs,
            },
            {
              header: "API value",
              numeric: true,
              cell: (row) => <Amount micros={row.api_value_micros} currency={currency} />,
              sortValue: (row) => row.api_value_micros,
            },
          ]}
        />
      }
    >
      <ResponsiveChart height={260}>
        {(size) => (
          <ScatterChart
            width={size.width}
            height={size.height}
            title="API value against duration"
            margin={{ top: 8, right: 16, bottom: 4, left: 0 }}
          >
            <CartesianGrid stroke="var(--border)" />
            <XAxis
              type="number"
              dataKey="duration"
              name="Duration"
              {...AXIS}
              tickFormatter={(value: number) => formatDuration(value)}
            />
            <YAxis
              type="number"
              dataKey="value"
              name="API value"
              {...AXIS}
              width={52}
              tickFormatter={(value: number) => compactMoney(value, currency)}
            />
            <Tooltip
              formatter={(value: unknown, name: unknown) =>
                typeof value !== "number"
                  ? String(value)
                  : name === "Duration"
                    ? formatDuration(value)
                    : formatCurrencyMicros(value, currency)
              }
            />
            <Scatter
              name="Sessions"
              data={data}
              fill={chartColor(0)}
              cursor="pointer"
              isAnimationActive={animate}
              onClick={(_, index) => {
                const row = points[index];
                if (row !== undefined) {
                  onOpenSession(row.agent, row.session);
                }
              }}
            />
          </ScatterChart>
        )}
      </ResponsiveChart>
    </ChartFrame>
  );
}

function CacheHitDistribution({ rows }: { readonly rows: readonly SessionPoint[] }): ReactNode {
  const bins = cacheHitBins(rows);
  return (
    <ChartFrame
      interactive
      title="Cache-hit ratio"
      description="Sessions per cache-hit band: cache reads over every input token. Sessions without input are left out."
      points={bins.length}
      table={
        <DataTable
          caption="Sessions per cache-hit band"
          rows={bins}
          rowKey={(row) => row.label}
          columns={COUNT_COLUMNS("Cache hits")}
        />
      }
    >
      <ColumnChart
        title="Cache-hit ratio"
        data={bins}
        name="Sessions"
        formatValue={formatCount}
        color={chartColor(1)}
      />
    </ChartFrame>
  );
}

function TokenMixChart({ rows }: { readonly rows: readonly SessionPoint[] }): ReactNode {
  const top = topRows(rows, TOP);
  const bars = tokenMixRows(top);
  return (
    <ChartFrame
      interactive
      title="Token mix of the costliest sessions"
      description={`Input, cache and output tokens of the ${formatCount(top.length)} sessions with the highest API value.`}
      points={bars.length * TOKEN_MIX_SERIES.length}
      table={
        <DataTable
          caption="Token mix of the costliest sessions"
          rows={bars}
          rowKey={(row) => row.label}
          columns={[
            { header: "Session", cell: (row) => row.label },
            ...TOKEN_MIX_SERIES.map(
              (series, index): Column<(typeof bars)[number]> => ({
                header: series.name,
                numeric: true,
                cell: (row) => formatTokens(row.values[index] ?? 0),
                sortValue: (row) => row.values[index] ?? 0,
              }),
            ),
          ]}
        />
      }
    >
      <HorizontalBarChart
        title="Token mix of the costliest sessions"
        rows={bars}
        series={TOKEN_MIX_SERIES}
        formatValue={formatTokens}
        stacked
      />
    </ChartFrame>
  );
}

function SessionsPerDay({ days }: { readonly days: SessionAnalytics["days"] }): ReactNode {
  const columns = days.map((day) => ({ label: day.day, value: day.sessions }));
  return (
    <ChartFrame
      interactive
      title="Sessions per day"
      description="Sessions started each day, in your time zone."
      points={columns.length}
      table={
        <DataTable
          caption="Sessions per day"
          rows={columns}
          rowKey={(row) => row.label}
          columns={COUNT_COLUMNS("Day")}
        />
      }
    >
      <ColumnChart
        title="Sessions per day"
        data={columns}
        name="Sessions"
        formatValue={formatCount}
        color={chartColor(2)}
      />
    </ChartFrame>
  );
}

function rankingCard(title: string, list: ReactNode, empty: string, count: number): ReactNode {
  return (
    <Card title={title}>
      {count === 0 ? <p className="text-sm text-fg-muted">{empty}</p> : list}
    </Card>
  );
}

/** The session charts of one filter set; `onOpenSession` follows a scatter dot. */
export function SessionAnalysis({
  data,
  onOpenSession,
}: {
  readonly data: SessionAnalytics;
  readonly onOpenSession: OpenSession;
}): ReactNode {
  const errors = toolErrorRanking(data.rows, TOP);
  const costly = topRows(data.rows, TOP);
  const maxCost = costly[0]?.api_value_micros ?? 0;
  return (
    <div className="flex flex-col gap-4">
      {data.rows.length < data.total ? (
        <p className="text-sm text-fg-muted">
          Per-session charts show the {formatCount(data.rows.length)} sessions with the highest API
          value out of {formatCount(data.total)}; the counts cover every session.
        </p>
      ) : null}
      <div className="grid gap-4 lg:grid-cols-2">
        {rankingCard(
          "Top sessions by API value",
          <RankedBars
            label="Top sessions by API value"
            items={costly.map((row) => ({
              key: `${row.agent}:${row.session}`,
              label: <SessionLink row={row} />,
              value: formatCurrencyMicros(row.api_value_micros, data.currency),
              percent: maxCost === 0 ? 0 : (100 * row.api_value_micros) / maxCost,
              detail: `${row.agent} · ${formatDuration(row.duration_secs)} · ${formatTokens(mixTotal(row.tokens))} tokens`,
            }))}
          />,
          "No session in range.",
          costly.length,
        )}
        {rankingCard(
          "Highest tool error rate",
          <RankedBars
            label="Highest tool error rate"
            items={errors.map(({ row, rate }) => ({
              key: `${row.agent}:${row.session}`,
              label: <SessionLink row={row} />,
              value: formatPercent(rate),
              percent: 100 * rate,
              detail: `${row.agent} · ${formatCount(row.tool_errors)} of ${formatCount(row.tool_calls)} tool calls failed`,
            }))}
          />,
          "No failed tool call in range.",
          errors.length,
        )}
        <CostHistogram data={data} />
        <CostScatter rows={data.rows} currency={data.currency} onOpenSession={onOpenSession} />
        <CacheHitDistribution rows={data.rows} />
        <TokenMixChart rows={data.rows} />
        <SessionsPerDay days={data.days} />
      </div>
      <Section
        title="Session starts by hour"
        description="Sessions started per weekday and hour, in your time zone. Darker means more."
      >
        <WeekHourTable
          label="Session starts by weekday and hour"
          grid={weekHourGrid(data.starts, (cell) => cell.sessions)}
          format={formatCount}
        />
      </Section>
    </div>
  );
}

/** Session analysis of the global filters. */
export function SessionAnalysisPanel(): ReactNode {
  const { filters } = useFilters();
  const navigate = useNavigate();
  const query = useSessionAnalytics(filters);
  return (
    <RemoteView
      remote={toRemote(query)}
      emptyWhen={(answer) =>
        answer.total === 0
          ? {
              title: "No sessions",
              hint: "No session matches these filters. Widen the range or clear a filter.",
            }
          : null
      }
    >
      {(data) => (
        <SessionAnalysis
          data={data}
          onOpenSession={(agent, session) =>
            void navigate({
              to: "/sessions/$agent/$session",
              params: { agent, session },
              search: (previous: Record<string, unknown>) => previous,
            })
          }
        />
      )}
    </RemoteView>
  );
}
