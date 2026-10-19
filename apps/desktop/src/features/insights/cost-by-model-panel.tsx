import type { ReactNode } from "react";
import type { BarDatum } from "../../shared/bar-chart";
import { compactMoney } from "../../shared/charts";
import type { Series } from "../../shared/schemas/usage";
import { StackedColumnChart } from "../../shared/stacked-column-chart";
import { TOP_MODELS } from "../../shared/top-models";
import { Amount, ChartFrame, type Column, DataTable, Section } from "../../shared/ui";
import { costByModel } from "./model";

const TITLE = "API value per model over time";

/** API value per day (week or month on long ranges), stacked by model. */
export function CostByModelPanel({ costs }: { readonly costs: Series }): ReactNode {
  const { series, rows } = costByModel(costs);
  const currency = costs.currency;
  const total = (row: BarDatum): number => row.values.reduce((sum, value) => sum + value, 0);
  const columns: Column<BarDatum>[] = [
    {
      header: `${costs.bucket[0]?.toUpperCase() ?? ""}${costs.bucket.slice(1)}`,
      cell: (row) => row.label,
    },
    ...series.map(
      (name, index): Column<BarDatum> => ({
        header: name,
        numeric: true,
        cell: (row) => <Amount micros={row.values[index] ?? 0} currency={currency} />,
        sortValue: (row) => row.values[index] ?? 0,
      }),
    ),
    {
      header: "Total",
      numeric: true,
      cell: (row) => <Amount micros={total(row)} currency={currency} />,
      sortValue: total,
    },
  ];
  return (
    <Section
      title="Cost by model over time"
      description={`API value per ${costs.bucket}, stacked by model (top ${TOP_MODELS}, the rest as Other). Extra usage has no model and is left out.`}
    >
      {series.length === 0 ? (
        <p className="text-sm text-fg-muted">No priced usage in this range.</p>
      ) : (
        <ChartFrame
          interactive
          title={TITLE}
          description={`${rows.length} points from ${rows[0]?.label ?? ""} to ${rows.at(-1)?.label ?? ""}.`}
          points={rows.length * series.length}
          table={
            <DataTable caption={TITLE} columns={columns} rows={rows} rowKey={(row) => row.label} />
          }
        >
          <StackedColumnChart
            title={TITLE}
            rows={rows}
            series={series}
            formatValue={(value) => compactMoney(value, currency)}
          />
        </ChartFrame>
      )}
    </Section>
  );
}
