import { type ReactNode, useState } from "react";
import { HorizontalBarChart } from "../../shared/bar-chart";
import { chartColor } from "../../shared/charts";
import { formatCount, formatCurrencyMicros, formatTokens } from "../../shared/format";
import type { SessionModel, SessionTimeline } from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
import { Amount, ChartFrame, type Column, DataTable } from "../../shared/ui";
import { mixTotal, TOKEN_MIX_SERIES, tokenMixValues } from "./analysis-model";

type Measure = "value" | "tokens";

const MEASURES = [
  { value: "value", label: "API value" },
  { value: "tokens", label: "Tokens" },
] as const;

function columns(currency: SessionTimeline["currency"]): Column<SessionModel>[] {
  return [
    { header: "Model", cell: (row) => row.model, sortValue: (row) => row.model },
    {
      header: "API value",
      numeric: true,
      cell: (row) => (
        <Amount micros={row.api_value_micros} currency={currency} approximate={row.approximate} />
      ),
      sortValue: (row) => row.api_value_micros,
    },
    {
      header: "Events",
      numeric: true,
      cell: (row) => formatCount(row.events),
      sortValue: (row) => row.events,
    },
    ...TOKEN_MIX_SERIES.map(
      (series, index): Column<SessionModel> => ({
        header: series.name,
        numeric: true,
        cell: (row) => formatTokens(tokenMixValues(row.tokens)[index] ?? 0),
        sortValue: (row) => tokenMixValues(row.tokens)[index] ?? 0,
      }),
    ),
    {
      header: "Tokens",
      numeric: true,
      cell: (row) => formatTokens(mixTotal(row.tokens)),
      sortValue: (row) => mixTotal(row.tokens),
    },
  ];
}

/** The session's usage per model: one bar per model, by API value or by token kind. */
export function SessionModelsPanel({
  timeline,
}: {
  readonly timeline: SessionTimeline;
}): ReactNode {
  const [measure, setMeasure] = useState<Measure>("value");
  const currency = timeline.currency;
  const title = measure === "value" ? "API value per model" : "Tokens per model";
  return (
    <ChartFrame
      interactive
      title="Usage per model"
      description={
        measure === "value"
          ? "API value of the session per model, highest first, over every event of the session."
          : "Tokens of the session per model, stacked by kind; output excludes reasoning."
      }
      points={timeline.models.length}
      actions={
        <SegmentedControl
          label="Measure models by"
          options={MEASURES}
          value={measure}
          onChange={setMeasure}
        />
      }
      table={
        <DataTable
          caption="Usage per model by kind"
          rows={timeline.models}
          rowKey={(row) => row.model}
          columns={columns(currency)}
        />
      }
    >
      {measure === "value" ? (
        <HorizontalBarChart
          title={title}
          rows={timeline.models.map((row) => ({
            label: row.model,
            values: [row.api_value_micros],
          }))}
          series={[{ name: "API value", color: chartColor(0) }]}
          formatValue={(value) => formatCurrencyMicros(value, currency)}
        />
      ) : (
        <HorizontalBarChart
          title={title}
          rows={timeline.models.map((row) => ({
            label: row.model,
            values: tokenMixValues(row.tokens),
          }))}
          series={TOKEN_MIX_SERIES}
          formatValue={formatTokens}
          stacked
        />
      )}
    </ChartFrame>
  );
}
