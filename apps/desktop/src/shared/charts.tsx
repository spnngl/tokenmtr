import type { ReactNode } from "react";
import {
  Bar,
  CartesianGrid,
  ComposedChart,
  Legend,
  Line,
  LineChart,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { Currency } from "./format";
import { formatCurrencyMicros } from "./format";
import type { QuotaHistory, Series } from "./schemas";

/**
 * Charts are SVG, decorative to assistive tech (`ChartFrame` pairs each with a table), and never
 * animate (D47: no animation while hidden or under reduced motion; none is simplest to keep so).
 * Backends send at most ~400 points, so nothing is thinned here.
 */

const CHART_COLORS = 8;

/** The CSS variable of the n-th chart colour; wraps after eight. */
export function chartColor(index: number): string {
  return `var(--chart-${(index % CHART_COLORS) + 1})`;
}

const AXIS = { stroke: "var(--border)", tick: { fill: "var(--fg-muted)", fontSize: 11 } } as const;

function compactMoney(micros: number, currency: Currency): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency,
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(micros / 1_000_000);
}

interface CostChartProps {
  readonly series: Series;
  readonly currency: Currency;
}

/** Group keys of a series, biggest total first so the stack order is stable. */
export function seriesGroups(series: Series): string[] {
  const totals = new Map<string, number>();
  for (const point of series.points) {
    for (const [group, usage] of Object.entries(point.groups)) {
      totals.set(group, (totals.get(group) ?? 0) + usage.api_value_micros);
    }
  }
  return [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([group]) => group);
}

/** Daily API value stacked by group, with cumulative API value and paid cost as lines. */
export function CostChart({ series, currency }: CostChartProps): ReactNode {
  const groups = seriesGroups(series);
  const data = series.points.map((point) => ({
    start: point.start,
    ...Object.fromEntries(
      groups.map((group) => [group, point.groups[group]?.api_value_micros ?? 0]),
    ),
    cumulativeApiValue: point.cumulative_api_value_micros,
    cumulativePaid: point.cumulative_paid_micros,
  }));
  const tooltipFormat = (value: unknown): string =>
    typeof value === "number" ? formatCurrencyMicros(value, currency) : String(value);
  return (
    <ResponsiveContainer width="100%" height="100%">
      <ComposedChart data={data}>
        <CartesianGrid stroke="var(--border)" vertical={false} />
        <XAxis dataKey="start" {...AXIS} minTickGap={32} />
        <YAxis
          yAxisId="day"
          {...AXIS}
          tickFormatter={(v: number) => compactMoney(v, currency)}
          width={52}
        />
        <YAxis
          yAxisId="total"
          orientation="right"
          {...AXIS}
          tickFormatter={(v: number) => compactMoney(v, currency)}
          width={52}
        />
        <Tooltip formatter={tooltipFormat} />
        <Legend />
        {groups.map((group, index) => (
          <Bar
            key={group}
            yAxisId="day"
            dataKey={group}
            stackId="cost"
            fill={chartColor(index)}
            isAnimationActive={false}
          />
        ))}
        <Line
          yAxisId="total"
          dataKey="cumulativeApiValue"
          name="Cumulative API value"
          stroke="var(--fg)"
          dot={false}
          strokeWidth={2}
          isAnimationActive={false}
        />
        <Line
          yAxisId="total"
          dataKey="cumulativePaid"
          name="Cumulative paid"
          stroke="var(--fg-muted)"
          strokeDasharray="4 3"
          dot={false}
          strokeWidth={2}
          isAnimationActive={false}
        />
      </ComposedChart>
    </ResponsiveContainer>
  );
}

/** Utilization of one quota window over time; each reset is a vertical marker. */
export function QuotaHistoryChart({ history }: { readonly history: QuotaHistory }): ReactNode {
  const data = history.points.map((point) => ({
    at: Date.parse(point.at),
    used: point.used_percent,
  }));
  const formatTick = (at: number): string =>
    new Date(at).toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit" });
  return (
    <ResponsiveContainer width="100%" height="100%">
      <LineChart data={data}>
        <CartesianGrid stroke="var(--border)" vertical={false} />
        <XAxis
          dataKey="at"
          type="number"
          scale="time"
          domain={["dataMin", "dataMax"]}
          tickFormatter={formatTick}
          {...AXIS}
          minTickGap={48}
        />
        <YAxis
          domain={[0, 100]}
          tickFormatter={(v: number) => `${Math.round(v)}%`}
          {...AXIS}
          width={44}
        />
        <Tooltip
          labelFormatter={(at) => (typeof at === "number" ? formatTick(at) : String(at))}
          formatter={(v: unknown) => `${String(v)}%`}
        />
        <ReferenceLine y={90} stroke="var(--danger)" strokeDasharray="4 3" />
        <ReferenceLine y={70} stroke="var(--warning)" strokeDasharray="4 3" />
        {history.resets.map((reset) => (
          <ReferenceLine
            key={reset}
            x={Date.parse(reset)}
            stroke="var(--fg-muted)"
            strokeOpacity={0.4}
          />
        ))}
        <Line
          dataKey="used"
          name="Used"
          stroke="var(--chart-1)"
          dot={false}
          strokeWidth={2}
          isAnimationActive={false}
        />
      </LineChart>
    </ResponsiveContainer>
  );
}
