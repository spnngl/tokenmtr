import type { ReactNode } from "react";
import { Bar, BarChart, CartesianGrid, Legend, Tooltip, XAxis, YAxis } from "recharts";
import { chartColor } from "./charts";
import { ResponsiveChart } from "./responsive-chart";
import { useChartAnimation } from "./use-chart-animation";

/** One category and its value per series, in `series` order. */
export interface BarDatum {
  readonly label: string;
  readonly values: readonly number[];
}

interface BarSeries {
  readonly name: string;
  /** CSS colour; defaults to the chart colour at the series position. */
  readonly color?: string;
}

interface HorizontalBarChartProps {
  readonly rows: readonly BarDatum[];
  readonly series: readonly BarSeries[];
  readonly formatValue: (value: number) => string;
  /**
   * Names the chart and turns on Recharts' accessibility layer (keyboard-browsable tooltip);
   * pair it with `ChartFrame interactive`. Without it the chart is decoration only.
   */
  readonly title?: string;
  /** Stacks the series in one bar per row. */
  readonly stacked?: boolean;
}

const ROW_HEIGHT = 30;
const AXIS_WIDTH = 150;
const AXIS_STYLE = { fontSize: 12, fill: "var(--fg-muted)" } as const;

/** Flat record for Recharts: `{ label, s0, s1, … }`. */
function toRecords(rows: readonly BarDatum[]): readonly Record<string, string | number>[] {
  return rows.map((row) => ({
    label: row.label,
    ...Object.fromEntries(row.values.map((value, index) => [`s${index}`, value])),
  }));
}

function formatTooltip(formatValue: (value: number) => string) {
  return (value: unknown): string =>
    typeof value === "number" ? formatValue(value) : String(value);
}

/**
 * Horizontal bars, one group (or stack) per row. Pair it with a `ChartFrame` whose table carries
 * the values.
 */
export function HorizontalBarChart({
  rows,
  series,
  formatValue,
  title,
  stacked = false,
}: HorizontalBarChartProps): ReactNode {
  const animate = useChartAnimation();
  const data = toRecords(rows);
  const perRow = stacked ? 1 : Math.max(1, series.length * 0.7);
  const height = rows.length * ROW_HEIGHT * perRow + 40;
  return (
    <ResponsiveChart height={Math.round(height)}>
      {(size) => (
        <BarChart
          width={size.width}
          height={size.height}
          data={data}
          layout="vertical"
          accessibilityLayer={title !== undefined}
          {...(title === undefined ? {} : { title })}
          margin={{ top: 4, right: 16, bottom: 4, left: 0 }}
        >
          <CartesianGrid horizontal={false} stroke="var(--border)" />
          <XAxis
            type="number"
            allowDecimals={false}
            tick={AXIS_STYLE}
            tickFormatter={formatValue}
            stroke="var(--border)"
          />
          <YAxis
            type="category"
            dataKey="label"
            interval={0}
            width={AXIS_WIDTH}
            tick={AXIS_STYLE}
            stroke="var(--border)"
          />
          <Tooltip formatter={formatTooltip(formatValue)} />
          {series.length > 1 ? <Legend itemSorter={null} /> : null}
          {series.map((entry, index) => (
            <Bar
              key={entry.name}
              name={entry.name}
              dataKey={`s${index}`}
              fill={entry.color ?? chartColor(index)}
              {...(stacked ? { stackId: "row" } : {})}
              isAnimationActive={animate}
            />
          ))}
        </BarChart>
      )}
    </ResponsiveChart>
  );
}
