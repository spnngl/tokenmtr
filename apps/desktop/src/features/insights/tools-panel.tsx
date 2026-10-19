import { type ReactNode, useState } from "react";
import { HorizontalBarChart } from "../../shared/bar-chart";
import { topRows } from "../../shared/decimate";
import { formatCount, formatPercent } from "../../shared/format";
import type { ToolRow, ToolsReport } from "../../shared/schemas/usage";
import { SegmentedControl } from "../../shared/segmented-control";
import { TOP_MODELS } from "../../shared/top-models";
import { ChartFrame, type Column, DataTable, Section } from "../../shared/ui";
import { type ToolGrouping, toolCallsByModel, toolKey, toolLabel } from "./model";

const GROUPINGS = [
  { value: "tool", label: "By tool" },
  { value: "server", label: "By MCP server" },
] as const;

const CHART_ROWS = 10;

function columns(
  series: readonly string[],
  split: (row: ToolRow) => readonly number[],
): Column<ToolRow>[] {
  return [
    { header: "Name", cell: (row) => <span className="font-medium">{toolLabel(row)}</span> },
    ...series.map(
      (name, index): Column<ToolRow> => ({
        header: name,
        numeric: true,
        cell: (row) => formatCount(split(row)[index] ?? 0),
      }),
    ),
    { header: "Calls", numeric: true, cell: (row) => formatCount(row.calls) },
    { header: "Errors", numeric: true, cell: (row) => formatCount(row.errors) },
    {
      header: "Error rate",
      numeric: true,
      cell: (row) => formatPercent(row.error_rate_percent / 100, 1),
    },
    { header: "Sessions", numeric: true, cell: (row) => formatCount(row.sessions) },
  ];
}

/** Tool calls by tool or by MCP server, stacked by model, with errors and error rate. */
export function ToolsPanel({ report }: { readonly report: ToolsReport }): ReactNode {
  const [grouping, setGrouping] = useState<ToolGrouping>("tool");
  const rows = grouping === "tool" ? report.tools : report.mcp_servers;
  const plotted = topRows(rows, CHART_ROWS);
  const { series, values } = toolCallsByModel(report.by_model, grouping);
  const split = (row: ToolRow): readonly number[] =>
    values.get(toolKey(row, grouping)) ?? series.map(() => 0);
  const title = grouping === "tool" ? "Calls per tool by model" : "Calls per MCP server by model";
  return (
    <Section
      title="Tool calls"
      description="Busiest first. MCP tools are shown as server / tool."
      actions={
        <SegmentedControl
          label="Group tool calls"
          options={GROUPINGS}
          value={grouping}
          onChange={setGrouping}
        />
      }
    >
      {rows.length === 0 ? (
        <p className="text-sm text-fg-muted">No tool calls in this range.</p>
      ) : (
        <ChartFrame
          interactive
          title={title}
          description={`The ${plotted.length} busiest of ${rows.length}, calls stacked by model (top ${TOP_MODELS}, the rest as Other). A call counts under the model its session used last before it; errors are in the table.`}
          points={plotted.length * series.length}
          table={
            <DataTable
              caption={grouping === "tool" ? "Tool calls per tool" : "Tool calls per MCP server"}
              columns={columns(series, split)}
              rows={rows}
              rowKey={(row) => toolKey(row)}
            />
          }
        >
          <HorizontalBarChart
            title={title}
            rows={plotted.map((row) => ({ label: toolLabel(row), values: split(row) }))}
            series={series.map((name) => ({ name }))}
            formatValue={(value) => formatCount(value)}
            stacked
          />
        </ChartFrame>
      )}
    </Section>
  );
}
