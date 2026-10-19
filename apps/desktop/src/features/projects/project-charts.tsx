import type { ReactNode } from "react";
import { CartesianGrid, Legend, Line, LineChart, Tooltip, XAxis, YAxis } from "recharts";
import { type BarDatum, HorizontalBarChart } from "../../shared/bar-chart";
import { AXIS, chartColor, compactMoney } from "../../shared/charts";
import { topRows } from "../../shared/decimate";
import {
  type Currency,
  formatCurrencyMicros,
  formatPercent,
  formatTokens,
} from "../../shared/format";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { ModelRow, ProjectModels, ProjectRow, Series } from "../../shared/schemas";
import { OTHER_MODEL, splitRows, topModels } from "../../shared/top-models";
import { Amount, ChartFrame, type Column, DataTable } from "../../shared/ui";
import { useChartAnimation } from "../../shared/use-chart-animation";
import { chartModels, foldModels, type ModelTotals } from "../models/model";

/** Projects drawn in the per-model bars; the table lists them all. */
const PROJECT_BARS = 10;
/** Projects drawn as lines; the rest add up to one Other line. */
const PROJECT_LINES = 8;
/** The series key of usage without a project (the backend's `NO_GROUP`). */
const NO_PROJECT_KEY = "(none)";
const NO_PROJECT = "No project";

/** What a project is called on screen: its map name, else its key. */
export function projectLabel(row: Pick<ProjectRow, "project" | "name">): string {
  return row.name ?? row.project ?? NO_PROJECT;
}

/** Labels of the project series keys, from the projects breakdown under the same filters. */
export function seriesLabels(rows: readonly ProjectRow[]): (key: string) => string {
  const names = new Map(rows.map((row) => [row.project ?? NO_PROJECT_KEY, projectLabel(row)]));
  return (key) => names.get(key) ?? (key === NO_PROJECT_KEY ? NO_PROJECT : key);
}

const sum = (values: readonly number[]): number =>
  values.reduce((total, value) => total + value, 0);

/** API value per series of `top`, in `top` order. */
function valuesOf(models: readonly ModelTotals[], top: readonly string[]): number[] {
  const folded = foldModels(models, top);
  return top.map((model) => folded.find((row) => row.model === model)?.api_value_micros ?? 0);
}

/** One column per series, then the row total: the table twin of a stacked or multi-line chart. */
function seriesColumns(
  first: Column<BarDatum>,
  names: readonly string[],
  currency: Currency,
): Column<BarDatum>[] {
  const total = (row: BarDatum): number => sum(row.values);
  return [
    first,
    ...names.map(
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
}

/** Each project's API value split by model (top five models, the rest as Other). */
export function ProjectModelBars({ split }: { readonly split: ProjectModels }): ReactNode {
  const top = chartModels(split.rows.flatMap((row) => row.models));
  const rows = split.rows.map(
    (row): BarDatum => ({ label: projectLabel(row), values: valuesOf(row.models, top) }),
  );
  const drawn = topRows(rows, PROJECT_BARS);
  const title = "Cost by model per project";
  return (
    <ChartFrame
      interactive
      title={title}
      description={`API value of the ${drawn.length} costliest projects, stacked by model; models past the top five fold into Other. The table lists every project.`}
      points={drawn.length * top.length}
      table={
        <DataTable
          caption={title}
          rows={rows}
          // Two projects may share a display name.
          rowKey={(row, index) => `${index}:${row.label}`}
          columns={seriesColumns(
            { header: "Project", cell: (row) => row.label, sortValue: (row) => row.label },
            top,
            split.currency,
          )}
        />
      }
    >
      <HorizontalBarChart
        title={title}
        stacked
        rows={drawn}
        series={top.map((name) => ({ name }))}
        formatValue={(micros) => compactMoney(micros, split.currency)}
      />
    </ChartFrame>
  );
}

/** API value per bucket, one line per costly project and one for the rest. */
export function ProjectCostLines({
  series,
  label,
}: {
  readonly series: Series;
  /** Display name of a project series key. */
  readonly label: (key: string) => string;
}): ReactNode {
  const animate = useChartAnimation();
  const entries = series.points.flatMap((point) =>
    Object.entries(point.groups).map(([key, group]) => ({
      row: point.start,
      model: key,
      value: group.api_value_micros,
    })),
  );
  const top = topModels(
    entries.map((entry): readonly [string, number] => [entry.model, entry.value]),
    PROJECT_LINES,
  );
  const nameOf = (key: string): string => (key === OTHER_MODEL ? OTHER_MODEL : label(key));
  const values = splitRows(entries, top, (entry) => entry);
  const rows = series.points.map(
    (point): BarDatum => ({
      label: point.start,
      values: values.get(point.start) ?? top.map(() => 0),
    }),
  );
  const data = rows.map((row) => ({
    label: row.label,
    ...Object.fromEntries(row.values.map((value, index) => [`s${index}`, value])),
  }));
  const currency = series.currency;
  const title = "Cost over time";
  return (
    <ChartFrame
      interactive
      title={title}
      description={`API value per ${series.bucket} of the ${PROJECT_LINES} costliest projects; the rest add up to Other.`}
      points={rows.length * top.length}
      table={
        <DataTable
          caption="Cost over time per project"
          rows={rows}
          rowKey={(row) => row.label}
          columns={seriesColumns(
            { header: "Start", cell: (row) => row.label, sortValue: (row) => row.label },
            top.map(nameOf),
            currency,
          )}
        />
      }
    >
      <ResponsiveChart height={260}>
        {(size) => (
          <LineChart
            width={size.width}
            height={size.height}
            data={data}
            title={title}
            margin={{ top: 8, right: 16, bottom: 4, left: 0 }}
          >
            <CartesianGrid vertical={false} stroke="var(--border)" />
            <XAxis dataKey="label" {...AXIS} minTickGap={24} />
            <YAxis
              {...AXIS}
              tickFormatter={(micros: number) => compactMoney(micros, currency)}
              width={56}
            />
            <Tooltip
              formatter={(value: unknown) =>
                typeof value === "number" ? formatCurrencyMicros(value, currency) : String(value)
              }
            />
            <Legend itemSorter={null} />
            {top.map((key, index) => (
              <Line
                key={key}
                name={nameOf(key)}
                dataKey={`s${index}`}
                stroke={chartColor(index)}
                strokeWidth={2}
                dot={false}
                isAnimationActive={animate}
              />
            ))}
          </LineChart>
        )}
      </ResponsiveChart>
    </ChartFrame>
  );
}

/** One project's API value per model (top five, the rest as Other). */
export function ModelCostBars({
  rows,
  currency,
}: {
  readonly rows: readonly ModelRow[];
  readonly currency: Currency;
}): ReactNode {
  const totals = foldModels(rows, chartModels(rows));
  const whole = sum(totals.map((row) => row.api_value_micros));
  const share = (row: ModelTotals): number => (whole === 0 ? 0 : row.api_value_micros / whole);
  const title = "Cost by model";
  return (
    <ChartFrame
      interactive
      title={title}
      description="API value of this project per model; models past the top five fold into Other."
      points={totals.length}
      table={
        <DataTable
          caption={title}
          rows={totals}
          rowKey={(row) => row.model}
          columns={[
            { header: "Model", cell: (row) => row.model, sortValue: (row) => row.model },
            {
              header: "Tokens",
              numeric: true,
              cell: (row) => formatTokens(row.total_tokens),
              sortValue: (row) => row.total_tokens,
            },
            {
              header: "API value",
              numeric: true,
              cell: (row) => (
                <Amount
                  micros={row.api_value_micros}
                  currency={currency}
                  approximate={row.approximate}
                />
              ),
              sortValue: (row) => row.api_value_micros,
            },
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
      <HorizontalBarChart
        title={title}
        rows={totals.map((row) => ({ label: row.model, values: [row.api_value_micros] }))}
        series={[{ name: "API value" }]}
        formatValue={(micros) => compactMoney(micros, currency)}
      />
    </ChartFrame>
  );
}
