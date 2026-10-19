import type { ReactNode } from "react";
import { Bar, BarChart, CartesianGrid, Legend, Tooltip, XAxis, YAxis } from "recharts";
import type { BarDatum } from "./bar-chart";
import { AXIS, chartColor } from "./charts";
import { ResponsiveChart } from "./responsive-chart";
import { useChartAnimation } from "./use-chart-animation";

/**
 * Vertical columns, one stack of `series` per category (per-model histograms and days).
 * Recharts' accessibility layer is on and named by `title`: pair it with `ChartFrame interactive`.
 */
export function StackedColumnChart({
  title,
  rows,
  series,
  formatValue,
  height = 240,
}: {
  readonly title: string;
  /** Values in `series` order. */
  readonly rows: readonly BarDatum[];
  /** Series names, bottom of the stack first; coloured by position. */
  readonly series: readonly string[];
  readonly formatValue: (value: number) => string;
  readonly height?: number;
}): ReactNode {
  const animate = useChartAnimation();
  const data = rows.map((row) => ({
    label: row.label,
    ...Object.fromEntries(row.values.map((value, index) => [`s${index}`, value])),
  }));
  return (
    <ResponsiveChart height={height}>
      {(size) => (
        <BarChart
          width={size.width}
          height={size.height}
          data={data}
          title={title}
          margin={{ top: 4, right: 8, bottom: 4, left: 0 }}
        >
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis dataKey="label" {...AXIS} minTickGap={8} />
          <YAxis {...AXIS} tickFormatter={formatValue} width={56} allowDecimals={false} />
          <Tooltip
            formatter={(value: unknown) =>
              typeof value === "number" ? formatValue(value) : String(value)
            }
          />
          {/* Rank order, as stacked: largest model first, Other last. */}
          <Legend itemSorter={null} />
          {series.map((name, index) => (
            <Bar
              key={name}
              name={name}
              dataKey={`s${index}`}
              stackId="column"
              fill={chartColor(index)}
              isAnimationActive={animate}
            />
          ))}
        </BarChart>
      )}
    </ResponsiveChart>
  );
}
