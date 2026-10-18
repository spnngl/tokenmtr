import { mergeBuckets } from "../../shared/decimate";
import type { Currency } from "../../shared/format";
import type {
  BranchRow,
  HeatCell,
  Heatmap,
  SignalsReport,
  ToolRow,
  ToolsReport,
} from "../../shared/schemas/usage";

export type SignalDay = SignalsReport["days"][number];

/** Everything the Insights screen renders. */
export interface InsightsData {
  readonly currency: Currency;
  readonly tools: ToolsReport;
  readonly signals: SignalsReport;
  readonly branches: readonly BranchRow[];
  readonly heatmap: Heatmap;
}

/** True when no panel has anything to show. */
export function isInsightsEmpty(data: InsightsData): boolean {
  return (
    data.tools.tools.length === 0 &&
    data.tools.mcp_servers.length === 0 &&
    data.signals.api_errors === 0 &&
    data.signals.compactions === 0 &&
    data.branches.length === 0 &&
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

export const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;

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

export interface HeatGrid {
  /** Seven rows (Monday first) of 24 hourly values. */
  readonly rows: readonly (readonly number[])[];
  readonly max: number;
}

/** Lays the cells out as weekday rows by hour columns; missing cells are zero. */
export function heatGrid(cells: readonly HeatCell[], metric: HeatMetric): HeatGrid {
  const rows = WEEKDAYS.map(() => new Array<number>(24).fill(0));
  for (const cell of cells) {
    const row = rows[cell.weekday - 1];
    if (row !== undefined) {
      row[cell.hour] = heatValue(cell, metric);
    }
  }
  const max = Math.max(0, ...rows.flat());
  return { rows, max };
}

/** Fill strength 0-1 of a cell; zero activity is exactly 0. */
export function heatIntensity(value: number, max: number): number {
  return max <= 0 || value <= 0 ? 0 : value / max;
}
