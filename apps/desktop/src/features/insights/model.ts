import { mergeBuckets } from "../../shared/decimate";
import type {
  Branches,
  BranchRow,
  HeatCell,
  Heatmap,
  SignalsReport,
  ToolRow,
  ToolsReport,
} from "../../shared/schemas/usage";
import { type WeekHourGrid, weekHourGrid } from "../../shared/week-hour-table";

export type SignalDay = SignalsReport["days"][number];

/** Everything the Insights screen renders; each money-bearing report names its own currency. */
export interface InsightsData {
  readonly tools: ToolsReport;
  readonly signals: SignalsReport;
  readonly branches: Branches;
  readonly heatmap: Heatmap;
}

/** True when no panel has anything to show. */
export function isInsightsEmpty(data: InsightsData): boolean {
  return (
    data.tools.tools.length === 0 &&
    data.tools.mcp_servers.length === 0 &&
    data.signals.api_errors === 0 &&
    data.signals.compactions === 0 &&
    data.branches.rows.length === 0 &&
    data.heatmap.cells.every((cell) => cell.events === 0)
  );
}

/** "server / tool" for an MCP tool, the bare tool name otherwise, the server for server rows. */
export function toolLabel(row: ToolRow): string {
  if (row.mcp_server !== null && row.tool_name !== null) {
    return `${row.mcp_server} / ${row.tool_name}`;
  }
  return row.mcp_server ?? row.tool_name ?? "(unknown tool)";
}

/** Branch name, with a label for events recorded without one. */
export function branchLabel(row: BranchRow): string {
  return row.branch ?? "(no branch)";
}

/** Daily signal counts merged to at most `max` buckets; a merged bucket keeps its first date. */
export function signalPoints(days: readonly SignalDay[], max?: number): readonly SignalDay[] {
  return mergeBuckets(
    days,
    (first, next) => ({
      date: first.date,
      api_errors: first.api_errors + next.api_errors,
      compactions: first.compactions + next.compactions,
    }),
    max,
  );
}

export type HeatMetric = "events" | "total_tokens" | "api_value_micros";

/** The metric of one cell. */
function heatValue(cell: HeatCell, metric: HeatMetric): number {
  switch (metric) {
    case "events":
      return cell.events;
    case "total_tokens":
      return cell.total_tokens;
    case "api_value_micros":
      return cell.api_value_micros;
    default:
      return metric satisfies never;
  }
}

/** Lays the cells out as weekday rows by hour columns; missing cells are zero. */
export function heatGrid(cells: readonly HeatCell[], metric: HeatMetric): WeekHourGrid {
  return weekHourGrid(cells, (cell) => heatValue(cell, metric));
}
