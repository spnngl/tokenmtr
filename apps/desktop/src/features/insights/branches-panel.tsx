import type { ReactNode } from "react";
import { HorizontalBarChart } from "../../shared/bar-chart";
import { topRows } from "../../shared/decimate";
import type { Currency } from "../../shared/format";
import { formatCount, formatCurrencyMicros, formatTokens } from "../../shared/format";
import type { BranchRow } from "../../shared/schemas/usage";
import { Amount, ChartFrame, type Column, DataTable, Section } from "../../shared/ui";
import { branchLabel } from "./model";

const CHART_ROWS = 10;

function columns(currency: Currency): readonly Column<BranchRow>[] {
  return [
    { header: "Branch", cell: (row) => <span className="font-medium">{branchLabel(row)}</span> },
    { header: "Events", numeric: true, cell: (row) => formatCount(row.events) },
    { header: "Tokens", numeric: true, cell: (row) => formatTokens(row.total_tokens) },
    {
      header: "API value",
      numeric: true,
      cell: (row) => (
        <Amount micros={row.api_value_micros} currency={currency} approximate={row.approximate} />
      ),
    },
  ];
}

/** API value per git branch, biggest first. */
export function BranchesPanel({
  rows,
  currency,
}: {
  readonly rows: readonly BranchRow[];
  readonly currency: Currency;
}): ReactNode {
  const plotted = topRows(rows, CHART_ROWS);
  return (
    <Section title="Cost per branch" description="API value per git branch, biggest first.">
      {rows.length === 0 ? (
        <p className="text-sm text-fg-muted">No branch recorded in this range.</p>
      ) : (
        <ChartFrame
          title="API value per git branch"
          description={`The ${plotted.length} costliest of ${rows.length} branches.`}
          points={plotted.length}
          table={
            <DataTable
              caption="Usage per git branch"
              columns={columns(currency)}
              rows={rows}
              rowKey={(row) => row.branch ?? "\u0000"}
            />
          }
        >
          <HorizontalBarChart
            rows={plotted.map((row) => ({
              label: branchLabel(row),
              values: [row.api_value_micros],
            }))}
            series={[{ name: "API value" }]}
            formatValue={(value) => formatCurrencyMicros(value, currency)}
          />
        </ChartFrame>
      )}
    </Section>
  );
}
