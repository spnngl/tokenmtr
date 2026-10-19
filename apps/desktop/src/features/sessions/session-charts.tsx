import type { ReactNode } from "react";
import {
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { HorizontalBarChart } from "../../shared/bar-chart";
import { AXIS, chartColor, compactMoney } from "../../shared/charts";
import { mergeBuckets, topRows } from "../../shared/decimate";
import {
  type Currency,
  formatClock,
  formatCount,
  formatCurrencyMicros,
  formatDateTime,
  formatPercent,
  formatTokens,
  PLACEHOLDER,
} from "../../shared/format";
import { useSessionTimeline } from "../../shared/queries";
import { CardRemote } from "../../shared/remote-view";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { SessionTimeline, TimelineEvent } from "../../shared/schemas";
import { Amount, Card, ChartFrame, DataTable, Kpi, KpiGrid } from "../../shared/ui";
import { useChartAnimation } from "../../shared/use-chart-animation";
import { toRemote } from "../../shared/use-remote";
import {
  cacheHitRatio,
  mixTotal,
  sumTokenMix,
  TOKEN_MIX_SERIES,
  tokenMixValues,
} from "./analysis-model";
import { SessionModelsPanel } from "./session-models-panel";

/** Tools drawn in the tool chart; the table lists them all. */
const TOOL_BARS = 15;

const SIGNAL_STROKE = { api_error: "var(--danger)", compaction: "var(--warning)" } as const;

function CostOverTime({
  timeline,
  timeZone,
}: {
  readonly timeline: SessionTimeline;
  readonly timeZone: string;
}): ReactNode {
  const animate = useChartAnimation();
  // A merged point keeps the last event's running totals, so the lines still end on the totals.
  const points = mergeBuckets(timeline.events, (_: TimelineEvent, next: TimelineEvent) => next);
  const data = points.map((event) => ({
    at: Date.parse(event.at),
    api: event.cumulative_api_value_micros,
    paid: event.cumulative_paid_micros,
  }));
  const currency = timeline.currency;
  return (
    <ChartFrame
      interactive
      title="Cost over time"
      description="Running API value and running paid cost of the session. Red dashed lines mark API errors, amber lines context compactions."
      points={data.length}
      table={
        <DataTable
          caption="Cost over time"
          rows={points}
          rowKey={(row, index) => `${index}:${row.at}`}
          columns={[
            {
              header: "Time",
              cell: (row) => formatDateTime(Date.parse(row.at), timeZone),
              sortValue: (row) => row.at,
            },
            {
              header: "API value so far",
              numeric: true,
              cell: (row) => (
                <Amount micros={row.cumulative_api_value_micros} currency={currency} />
              ),
              sortValue: (row) => row.cumulative_api_value_micros,
            },
            {
              header: "Paid so far",
              numeric: true,
              cell: (row) => <Amount micros={row.cumulative_paid_micros} currency={currency} />,
              sortValue: (row) => row.cumulative_paid_micros,
            },
          ]}
        />
      }
    >
      {timeline.events_truncated ? (
        <p className="text-xs text-fg-muted">Only the first events of this session are drawn.</p>
      ) : null}
      <ResponsiveChart height={240}>
        {(size) => (
          <LineChart
            width={size.width}
            height={size.height}
            data={data}
            title="Cost over time"
            margin={{ top: 8, right: 16, bottom: 4, left: 0 }}
          >
            <CartesianGrid vertical={false} stroke="var(--border)" />
            <XAxis
              type="number"
              dataKey="at"
              scale="time"
              domain={["dataMin", "dataMax"]}
              {...AXIS}
              minTickGap={32}
              tickFormatter={(value: number) => formatClock(value, timeZone)}
            />
            <YAxis
              {...AXIS}
              width={52}
              tickFormatter={(value: number) => compactMoney(value, currency)}
            />
            <Tooltip
              labelFormatter={(value: unknown) =>
                typeof value === "number" ? formatDateTime(value, timeZone) : String(value)
              }
              formatter={(value: unknown) =>
                typeof value === "number" ? formatCurrencyMicros(value, currency) : String(value)
              }
            />
            <Legend />
            {timeline.signals.map((signal) => (
              <ReferenceLine
                key={`${signal.at}:${signal.kind}`}
                x={Date.parse(signal.at)}
                stroke={SIGNAL_STROKE[signal.kind]}
                strokeDasharray={signal.kind === "api_error" ? "4 3" : undefined}
              />
            ))}
            <Line
              dataKey="api"
              name="API value"
              stroke={chartColor(0)}
              strokeWidth={2}
              dot={false}
              isAnimationActive={animate}
            />
            <Line
              dataKey="paid"
              name="Paid"
              stroke={chartColor(1)}
              strokeWidth={2}
              dot={false}
              isAnimationActive={animate}
            />
          </LineChart>
        )}
      </ResponsiveChart>
    </ChartFrame>
  );
}

function TokenMixPanel({ values }: { readonly values: readonly number[] }): ReactNode {
  const total = values.reduce((sum, value) => sum + value, 0);
  const rows = TOKEN_MIX_SERIES.map((series, index) => ({
    label: series.name,
    values: [values[index] ?? 0],
  }));
  return (
    <ChartFrame
      interactive
      title="Token mix"
      description="Tokens of the session by kind; output excludes reasoning, which has its own bar."
      points={rows.length}
      table={
        <DataTable
          caption="Token mix"
          rows={rows}
          rowKey={(row) => row.label}
          columns={[
            { header: "Kind", cell: (row) => row.label },
            {
              header: "Tokens",
              numeric: true,
              cell: (row) => formatTokens(row.values[0] ?? 0),
              sortValue: (row) => row.values[0] ?? 0,
            },
            {
              header: "Share",
              numeric: true,
              cell: (row) =>
                total === 0 ? PLACEHOLDER : formatPercent((row.values[0] ?? 0) / total),
            },
          ]}
        />
      }
    >
      <HorizontalBarChart
        title="Token mix"
        rows={rows}
        series={[{ name: "Tokens" }]}
        formatValue={formatTokens}
      />
    </ChartFrame>
  );
}

function ToolsPanel({ tools }: { readonly tools: SessionTimeline["tools"] }): ReactNode {
  const label = (tool: SessionTimeline["tools"][number]): string =>
    tool.mcp_server === null ? tool.tool_name : `${tool.tool_name} (${tool.mcp_server})`;
  const rows = topRows(tools, TOOL_BARS).map((tool) => ({
    label: label(tool),
    values: [tool.calls - tool.failed, tool.failed],
  }));
  return (
    <ChartFrame
      interactive
      title="Tool calls"
      description="Calls per tool, most called first, split into succeeded and failed."
      points={rows.length * 2}
      table={
        <DataTable
          caption="Tool calls per tool"
          rows={tools}
          rowKey={(tool) => `${tool.mcp_server ?? ""}:${tool.tool_name}`}
          columns={[
            { header: "Tool", cell: label, sortValue: label },
            {
              header: "Calls",
              numeric: true,
              cell: (tool) => formatCount(tool.calls),
              sortValue: (tool) => tool.calls,
            },
            {
              header: "Failed",
              numeric: true,
              cell: (tool) => formatCount(tool.failed),
              sortValue: (tool) => tool.failed,
            },
            {
              header: "Failed share",
              numeric: true,
              cell: (tool) =>
                tool.calls === 0 ? PLACEHOLDER : formatPercent(tool.failed / tool.calls),
              sortValue: (tool) => (tool.calls === 0 ? 0 : tool.failed / tool.calls),
            },
          ]}
        />
      }
    >
      <HorizontalBarChart
        title="Tool calls"
        rows={rows}
        series={[
          { name: "Succeeded", color: chartColor(2) },
          { name: "Failed", color: "var(--danger)" },
        ]}
        formatValue={formatCount}
        stacked
      />
    </ChartFrame>
  );
}

function totals(timeline: SessionTimeline): { api: number; paid: number } {
  const last = timeline.events.at(-1);
  return {
    api: last?.cumulative_api_value_micros ?? 0,
    paid: last?.cumulative_paid_micros ?? 0,
  };
}

/**
 * Charts of one session: cost over time with signal markers, usage per model, token mix, cache
 * hits, tools.
 */
export function SessionCharts({
  timeline,
  timeZone,
}: {
  readonly timeline: SessionTimeline;
  readonly timeZone: string;
}): ReactNode {
  const mix = sumTokenMix(timeline.events.map((event) => event.tokens));
  const ratio = cacheHitRatio(mix);
  const sums = totals(timeline);
  const currency: Currency = timeline.currency;
  return (
    <Card title="Analysis">
      <KpiGrid>
        <Kpi label="API value" value={<Amount micros={sums.api} currency={currency} />} />
        <Kpi label="Paid" value={<Amount micros={sums.paid} currency={currency} />} />
        <Kpi label="Tokens" value={formatTokens(mixTotal(mix))} />
        <Kpi
          label="Cache-hit ratio"
          value={ratio === null ? PLACEHOLDER : formatPercent(ratio)}
          detail="Cache reads over every input token"
        />
      </KpiGrid>
      {timeline.events.length === 0 ? (
        <p className="text-sm text-fg-muted">No priced events in this session.</p>
      ) : (
        <CostOverTime timeline={timeline} timeZone={timeZone} />
      )}
      {timeline.models.length === 0 ? null : <SessionModelsPanel timeline={timeline} />}
      <div className="grid gap-4 lg:grid-cols-2">
        <TokenMixPanel values={tokenMixValues(mix)} />
        {timeline.tools.length === 0 ? (
          <p className="text-sm text-fg-muted">No tool calls recorded.</p>
        ) : (
          <ToolsPanel tools={timeline.tools} />
        )}
      </div>
    </Card>
  );
}

/** Loads and shows the charts of one session. */
export function SessionChartsPanel(props: {
  readonly agent: string;
  readonly session: string;
  readonly currency: Currency;
  readonly timeZone: string;
}): ReactNode {
  const remote = toRemote(useSessionTimeline(props.agent, props.session, props.currency));
  return (
    <CardRemote title="Analysis" remote={remote}>
      {(timeline) => <SessionCharts timeline={timeline} timeZone={props.timeZone} />}
    </CardRemote>
  );
}
