import type { ReactNode } from "react";
import { Bar, BarChart, CartesianGrid, Legend, Tooltip, XAxis, YAxis } from "recharts";
import { chartColor } from "../../shared/charts";
import { mergeBuckets } from "../../shared/decimate";
import type { Currency } from "../../shared/format";
import { formatCurrencyMicros } from "../../shared/format";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { BillingReport } from "../../shared/schemas";
import { Amount, ChartFrame, type Column, DataTable, Section } from "../../shared/ui";
import { useChartAnimation } from "../../shared/use-chart-animation";
import { formatRoi } from "./model";

type Month = BillingReport["months"][number];

const AXIS_STYLE = { fontSize: 12, fill: "var(--fg-muted)" } as const;

function monthColumns(currency: Currency): readonly Column<Month>[] {
  return [
    { header: "Month", cell: (row) => row.month.slice(0, 7) },
    {
      header: "Paid",
      numeric: true,
      cell: (row) => (
        <Amount micros={row.paid_micros} currency={currency} approximate={row.approximate} />
      ),
    },
    {
      header: "API value",
      numeric: true,
      cell: (row) => (
        <Amount micros={row.api_value_micros} currency={currency} approximate={row.approximate} />
      ),
    },
    { header: "ROI", numeric: true, cell: (row) => formatRoi(row.roi) },
  ];
}

function combine(first: Month, next: Month): Month {
  return {
    ...first,
    paid_micros: first.paid_micros + next.paid_micros,
    api_value_micros: first.api_value_micros + next.api_value_micros,
    approximate: first.approximate || next.approximate,
    roi: null,
  };
}

function MonthlyChart({
  months,
  currency,
}: {
  readonly months: readonly { month: string; paid: number; api: number }[];
  readonly currency: Currency;
}): ReactNode {
  const animate = useChartAnimation();
  return (
    <ResponsiveChart height={240}>
      {(size) => (
        <BarChart
          width={size.width}
          height={size.height}
          data={months}
          accessibilityLayer={false}
          margin={{ top: 4, right: 8, bottom: 4, left: 0 }}
        >
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis dataKey="month" tick={AXIS_STYLE} minTickGap={16} stroke="var(--border)" />
          <YAxis
            tick={AXIS_STYLE}
            width={64}
            stroke="var(--border)"
            tickFormatter={(value: number) => formatCurrencyMicros(value, currency)}
          />
          <Tooltip
            formatter={(value: unknown) =>
              typeof value === "number" ? formatCurrencyMicros(value, currency) : String(value)
            }
          />
          <Legend />
          <Bar name="Paid" dataKey="paid" fill={chartColor(0)} isAnimationActive={animate} />
          <Bar name="API value" dataKey="api" fill={chartColor(1)} isAnimationActive={animate} />
        </BarChart>
      )}
    </ResponsiveChart>
  );
}

/** Paid cost against API value per calendar month, with ROI. */
export function MonthlyPanel({ report }: { readonly report: BillingReport }): ReactNode {
  const months = mergeBuckets(report.months, combine);
  return (
    <Section
      title="Paid against API value"
      description="Per month. Paid is pay-as-you-go cost plus subscription fees; ROI is API value over fees."
    >
      {months.length === 0 ? (
        <p className="text-sm text-fg-muted">No usage in this range.</p>
      ) : (
        <ChartFrame
          title="Paid and API value per month"
          description={`${months.length} months from ${months[0]?.month.slice(0, 7) ?? ""} to ${months.at(-1)?.month.slice(0, 7) ?? ""}.`}
          points={months.length}
          table={
            <DataTable
              caption="Billing per month"
              columns={monthColumns(report.currency)}
              rows={months}
              rowKey={(row) => row.month}
            />
          }
        >
          <MonthlyChart
            currency={report.currency}
            months={months.map((row) => ({
              month: row.month.slice(0, 7),
              paid: row.paid_micros,
              api: row.api_value_micros,
            }))}
          />
        </ChartFrame>
      )}
    </Section>
  );
}
