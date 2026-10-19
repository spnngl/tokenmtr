import type { ReactNode } from "react";
import { Bar, BarChart, CartesianGrid, Tooltip, XAxis, YAxis } from "recharts";
import { AXIS, chartColor } from "./charts";
import { ResponsiveChart } from "./responsive-chart";
import { useChartAnimation } from "./use-chart-animation";

/** One column: its axis label and value. */
export interface ColumnDatum {
  readonly label: string;
  readonly value: number;
}

/**
 * Vertical bars, one per category (histograms, per-day counts). Recharts' accessibility layer is
 * on and named by `title`: pair it with `ChartFrame interactive`.
 */
export function ColumnChart({
  title,
  data,
  name,
  formatValue,
  height = 220,
  color = chartColor(0),
}: {
  readonly title: string;
  readonly data: readonly ColumnDatum[];
  /** Series name in the tooltip, e.g. "Sessions". */
  readonly name: string;
  readonly formatValue: (value: number) => string;
  readonly height?: number;
  readonly color?: string;
}): ReactNode {
  const animate = useChartAnimation();
  return (
    <ResponsiveChart height={height}>
      {(size) => (
        <BarChart
          width={size.width}
          height={size.height}
          data={[...data]}
          title={title}
          margin={{ top: 4, right: 8, bottom: 4, left: 0 }}
        >
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis dataKey="label" {...AXIS} minTickGap={8} />
          <YAxis {...AXIS} tickFormatter={formatValue} width={48} allowDecimals={false} />
          <Tooltip
            formatter={(value: unknown) =>
              typeof value === "number" ? formatValue(value) : String(value)
            }
          />
          <Bar dataKey="value" name={name} fill={color} isAnimationActive={animate} />
        </BarChart>
      )}
    </ResponsiveChart>
  );
}
