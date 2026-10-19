import type { BarDatum } from "../../shared/bar-chart";
import { mergeBuckets } from "../../shared/decimate";
import type {
  Branches,
  BranchRow,
  HeatCell,
  Heatmap,
  Series,
  SignalsReport,
  ToolModelCalls,
  ToolRow,
  ToolsReport,
} from "../../shared/schemas/usage";
import { seriesOf, splitRows, topModels, UNKNOWN_MODEL } from "../../shared/top-models";
import { type WeekHourGrid, weekHourGrid } from "../../shared/week-hour-table";

export type SignalDay = SignalsReport["days"][number];

/** Everything the Insights screen renders; each money-bearing report names its own currency. */
export interface InsightsData {
  readonly tools: ToolsReport;
  readonly signals: SignalsReport;
  readonly branches: Branches;
  readonly heatmap: Heatmap;
  /** Series grouped by model, for cost by model over time. */
  readonly costs: Series;
}

/** True when no panel has anything to show. */
export function isInsightsEmpty(data: InsightsData): boolean {
  return (
    data.tools.tools.length === 0 &&
    data.tools.mcp_servers.length === 0 &&
    data.signals.api_errors === 0 &&
    data.signals.compactions === 0 &&
    data.branches.rows.length === 0 &&
    data.heatmap.cells.every((cell) => cell.events === 0) &&
    data.costs.points.every((point) => Object.keys(point.groups).length === 0)
  );
}

export type ToolGrouping = "tool" | "server";

/** Identity of a tool row, or of a tool's calls under `grouping`. */
export function toolKey(
  row: Pick<ToolRow, "tool_name" | "mcp_server">,
  grouping: ToolGrouping = "tool",
): string {
  return grouping === "tool"
    ? `${row.mcp_server ?? ""}\u0000${row.tool_name ?? ""}`
    : `${row.mcp_server ?? ""}\u0000`;
}

/**
 * Calls per tool (or MCP server) split into the top models by calls plus "Other": the model
 * series and, per {@link toolKey}, the calls in series order.
 */
export function toolCallsByModel(
  calls: readonly ToolModelCalls[],
  grouping: ToolGrouping,
): { readonly series: readonly string[]; readonly values: ReadonlyMap<string, number[]> } {
  const own = grouping === "tool" ? calls : calls.filter((row) => row.mcp_server !== null);
  const model = (row: ToolModelCalls): string => row.model ?? UNKNOWN_MODEL;
  const series = topModels(own.map((row) => [model(row), row.calls] as const));
  const values = splitRows(own, series, (row) => ({
    row: toolKey(row, grouping),
    model: model(row),
    value: row.calls,
  }));
  return { series, values };
}

/** API errors and compactions of the top models by signals, then "Other". */
export function signalsByModel(report: SignalsReport): readonly BarDatum[] {
  const model = (row: SignalsReport["models"][number]): string => row.model ?? UNKNOWN_MODEL;
  const series = topModels(
    report.models.map((row) => [model(row), row.api_errors + row.compactions] as const),
  );
  const rows = new Map(series.map((name) => [name, [0, 0]]));
  for (const row of report.models) {
    const values = rows.get(seriesOf(model(row), series));
    if (values !== undefined) {
      values[0] = (values[0] ?? 0) + row.api_errors;
      values[1] = (values[1] ?? 0) + row.compactions;
    }
  }
  return [...rows].map(([label, values]) => ({ label, values }));
}

/**
 * API value per bucket split into the top models by API value plus "Other". Extra usage has no
 * model and is left out.
 */
export function costByModel(costs: Series): {
  readonly series: readonly string[];
  readonly rows: readonly BarDatum[];
} {
  const entries = costs.points.flatMap((point) =>
    Object.entries(point.groups).map(([model, group]) => ({
      start: point.start,
      model,
      value: group.api_value_micros,
    })),
  );
  const series = topModels(entries.map((entry) => [entry.model, entry.value] as const));
  const values = splitRows(entries, series, (entry) => ({
    row: entry.start,
    model: entry.model,
    value: entry.value,
  }));
  const rows = costs.points.map((point) => ({
    label: point.start,
    values: values.get(point.start) ?? series.map(() => 0),
  }));
  return { series, rows };
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
