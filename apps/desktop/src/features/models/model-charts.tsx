import type { ReactNode } from "react";
import { Legend, Pie, PieChart, Tooltip } from "recharts";
import { HorizontalBarChart } from "../../shared/bar-chart";
import { chartColor } from "../../shared/charts";
import {
  type Currency,
  formatCurrencyMicros,
  formatPercent,
  formatTokens,
  PLACEHOLDER,
} from "../../shared/format";
import { CardRemote, type Remote } from "../../shared/remote-view";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { Series } from "../../shared/schemas";
import { Amount, ChartFrame, type Column, DataTable } from "../../shared/ui";
import { useChartAnimation } from "../../shared/use-chart-animation";
import { CostByModelPanel } from "../insights/cost-by-model-panel";
import { cacheHitRatio, TOKEN_MIX_SERIES, tokenMixValues } from "../sessions/analysis-model";
import {
  chartModels,
  costPerMillion,
  foldModels,
  type ModelsData,
  type ModelTotals,
} from "./model";

const OTHER_NOTE = "Models past the top five fold into Other.";

const modelColumn: Column<ModelTotals> = {
  header: "Model",
  cell: (row) => <span className="font-medium">{row.model}</span>,
  sortValue: (row) => row.model,
};

function apiColumn(currency: Currency): Column<ModelTotals> {
  return {
    header: "API value",
    numeric: true,
    cell: (row) => (
      <Amount micros={row.api_value_micros} currency={currency} approximate={row.approximate} />
    ),
    sortValue: (row) => row.api_value_micros,
  };
}

function money(currency: Currency): (micros: number) => string {
  return (micros) => formatCurrencyMicros(micros, currency);
}

/** Each model's share of the API value, as a donut. */
export function CostShareChart({
  totals,
  currency,
}: {
  readonly totals: readonly ModelTotals[];
  readonly currency: Currency;
}): ReactNode {
  const animate = useChartAnimation();
  const whole = totals.reduce((sum, row) => sum + row.api_value_micros, 0);
  const share = (row: ModelTotals): number => (whole === 0 ? 0 : row.api_value_micros / whole);
  const data = totals.map((row, index) => ({
    label: row.model,
    value: row.api_value_micros,
    fill: chartColor(index),
  }));
  return (
    <ChartFrame
      interactive
      title="Cost share"
      description={`Share of the API value per model. ${OTHER_NOTE}`}
      points={data.length}
      table={
        <DataTable
          caption="Cost share per model"
          rows={totals}
          rowKey={(row) => row.model}
          columns={[
            modelColumn,
            apiColumn(currency),
            {
              header: "Share",
              numeric: true,
              cell: (row) => formatPercent(share(row), 1),
              sortValue: share,
            },
          ]}
        />
      }
    >
      <ResponsiveChart height={260}>
        {(size) => (
          <PieChart width={size.width} height={size.height} title="Cost share">
            <Pie
              data={data}
              dataKey="value"
              nameKey="label"
              innerRadius="50%"
              outerRadius="80%"
              stroke="var(--surface-raised)"
              isAnimationActive={animate}
            />
            <Tooltip
              formatter={(value: unknown) =>
                typeof value === "number" ? formatCurrencyMicros(value, currency) : String(value)
              }
            />
            <Legend itemSorter={null} />
          </PieChart>
        )}
      </ResponsiveChart>
    </ChartFrame>
  );
}

/** Input, cache and output tokens per model, stacked. */
export function TokenMixChart({ totals }: { readonly totals: readonly ModelTotals[] }): ReactNode {
  return (
    <ChartFrame
      interactive
      title="Token mix"
      description={`Tokens per model by kind; reasoning is the part of output spent thinking. ${OTHER_NOTE}`}
      points={totals.length}
      table={
        <DataTable
          caption="Token mix per model"
          rows={totals}
          rowKey={(row) => row.model}
          columns={[
            modelColumn,
            ...TOKEN_MIX_SERIES.map(
              (kind, index): Column<ModelTotals> => ({
                header: kind.name,
                numeric: true,
                cell: (row) => formatTokens(tokenMixValues(row)[index] ?? 0),
                sortValue: (row) => tokenMixValues(row)[index] ?? 0,
              }),
            ),
          ]}
        />
      }
    >
      <HorizontalBarChart
        title="Token mix"
        stacked
        rows={totals.map((row) => ({ label: row.model, values: tokenMixValues(row) }))}
        series={TOKEN_MIX_SERIES}
        formatValue={formatTokens}
      />
    </ChartFrame>
  );
}

/** Share of input tokens read from the cache, per model. */
export function CacheHitChart({ totals }: { readonly totals: readonly ModelTotals[] }): ReactNode {
  const rate = (row: ModelTotals): number => cacheHitRatio(row) ?? -1;
  const drawn = totals.flatMap((row) => {
    const ratio = cacheHitRatio(row);
    return ratio === null ? [] : [{ label: row.model, values: [ratio * 100] }];
  });
  return (
    <ChartFrame
      interactive
      title="Cache hit rate"
      description={`Cache reads over all input tokens, per model. ${OTHER_NOTE}`}
      points={drawn.length}
      table={
        <DataTable
          caption="Cache hit rate per model"
          rows={totals}
          rowKey={(row) => row.model}
          columns={[
            modelColumn,
            {
              header: "Cache read",
              numeric: true,
              cell: (row) => formatTokens(row.cache_read),
              sortValue: (row) => row.cache_read,
            },
            {
              header: "Hit rate",
              numeric: true,
              cell: (row) => {
                const ratio = cacheHitRatio(row);
                return ratio === null ? PLACEHOLDER : formatPercent(ratio, 1);
              },
              sortValue: rate,
            },
          ]}
        />
      }
    >
      <HorizontalBarChart
        title="Cache hit rate"
        rows={drawn}
        series={[{ name: "Cache hit rate" }]}
        formatValue={(percent) => formatPercent(percent / 100)}
      />
    </ChartFrame>
  );
}

/** API value per million tokens, per model: what a token of each model costs. */
export function CostPerMillionChart({
  totals,
  currency,
}: {
  readonly totals: readonly ModelTotals[];
  readonly currency: Currency;
}): ReactNode {
  const drawn = totals.flatMap((row) => {
    const cost = costPerMillion(row);
    return cost === null ? [] : [{ label: row.model, values: [cost] }];
  });
  return (
    <ChartFrame
      interactive
      title="Cost per million tokens"
      description={`API value over every token, cache reads included, per model. ${OTHER_NOTE}`}
      points={drawn.length}
      table={
        <DataTable
          caption="Cost per million tokens per model"
          rows={totals}
          rowKey={(row) => row.model}
          columns={[
            modelColumn,
            {
              header: "Tokens",
              numeric: true,
              cell: (row) => formatTokens(row.total_tokens),
              sortValue: (row) => row.total_tokens,
            },
            apiColumn(currency),
            {
              header: "Per million tokens",
              numeric: true,
              cell: (row) => {
                const cost = costPerMillion(row);
                return cost === null ? PLACEHOLDER : formatCurrencyMicros(cost, currency);
              },
              sortValue: (row) => costPerMillion(row) ?? -1,
            },
          ]}
        />
      }
    >
      <HorizontalBarChart
        title="Cost per million tokens"
        rows={drawn}
        series={[{ name: "Per million tokens" }]}
        formatValue={money(currency)}
      />
    </ChartFrame>
  );
}

/** The Models screen's charts: cost over time, then the per-model shares and ratios. */
export function ModelCharts({
  report,
  series,
}: {
  readonly report: ModelsData;
  readonly series: Remote<Series>;
}): ReactNode {
  const top = chartModels(report.rows);
  const totals = foldModels(report.rows, top);
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <div className="min-w-0 lg:col-span-2">
        <CardRemote title="Cost by model over time" remote={series}>
          {(data) => <CostByModelPanel costs={data} />}
        </CardRemote>
      </div>
      <CostShareChart totals={totals} currency={report.currency} />
      <CostPerMillionChart totals={totals} currency={report.currency} />
      <TokenMixChart totals={totals} />
      <CacheHitChart totals={totals} />
    </div>
  );
}
