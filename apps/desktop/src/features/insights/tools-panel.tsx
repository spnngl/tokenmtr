import { type ReactNode, useState } from "react";
import { HorizontalBarChart } from "../../shared/bar-chart";
import { topRows } from "../../shared/decimate";
import { formatPercent } from "../../shared/format";
import type { ToolRow, ToolsReport } from "../../shared/schemas/usage";
import { SegmentedControl } from "../../shared/segmented-control";
import { ChartFrame, type Column, DataTable, Section } from "../../shared/ui";
import { toolLabel } from "./model";

type Grouping = "tool" | "server";

const GROUPINGS = [
  { value: "tool", label: "By tool" },
  { value: "server", label: "By MCP server" },
] as const;

const CHART_ROWS = 10;

const COLUMNS: readonly Column<ToolRow>[] = [
  { header: "Name", cell: (row) => <span className="font-medium">{toolLabel(row)}</span> },
  { header: "Calls", numeric: true, cell: (row) => row.calls.toLocaleString("en-US") },
  { header: "Errors", numeric: true, cell: (row) => row.errors.toLocaleString("en-US") },
  {
    header: "Error rate",
    numeric: true,
    cell: (row) => formatPercent(row.error_rate_percent / 100, 1),
  },
  { header: "Sessions", numeric: true, cell: (row) => row.sessions.toLocaleString("en-US") },
];

function rowKey(row: ToolRow): string {
  return `${row.mcp_server ?? ""}\u0000${row.tool_name ?? ""}`;
}

/** Tool calls by tool or by MCP server, with call volume, errors and error rate. */
export function ToolsPanel({ report }: { readonly report: ToolsReport }): ReactNode {
  const [grouping, setGrouping] = useState<Grouping>("tool");
  const rows = grouping === "tool" ? report.tools : report.mcp_servers;
  const plotted = topRows(rows, CHART_ROWS);
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
          title={
            grouping === "tool" ? "Calls and errors per tool" : "Calls and errors per MCP server"
          }
          description={`The ${plotted.length} busiest of ${rows.length}: calls and errors.`}
          points={plotted.length}
          table={
            <DataTable
              caption={grouping === "tool" ? "Tool calls per tool" : "Tool calls per MCP server"}
              columns={COLUMNS}
              rows={rows}
              rowKey={rowKey}
            />
          }
        >
          <HorizontalBarChart
            rows={plotted.map((row) => ({
              label: toolLabel(row),
              values: [row.calls, row.errors],
            }))}
            series={[{ name: "Calls" }, { name: "Errors", color: "var(--danger)" }]}
            formatValue={(value) => value.toLocaleString("en-US")}
          />
        </ChartFrame>
      )}
    </Section>
  );
}
