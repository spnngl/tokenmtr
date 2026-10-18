import type { Range } from "../../shared/filters";
import type { Currency, IsoDate } from "../../shared/format";
import type { ReportFormat, ReportScope, ReportSpec } from "../../shared/schemas";

/**
 * What the Reports screen edits. The period stays a filter-bar `Range` (presets follow the
 * clock); `toReportSpec` pins it to the days the backend renders.
 */
export interface ReportRequest {
  readonly range: Range;
  readonly scope: ReportScope;
  readonly format: ReportFormat;
  readonly currency: Currency;
}

export function formatLabel(format: ReportFormat): string {
  switch (format) {
    case "markdown":
      return "Markdown";
    case "csv":
      return "CSV";
    case "html":
      return "HTML";
    default:
      return format satisfies never;
  }
}

export function rangeLabel(range: Range): string {
  switch (range.kind) {
    case "preset":
      return range.preset === "today" ? "Today" : `Last ${range.preset.replace("d", " days")}`;
    case "custom":
      return `${range.from} to ${range.to}`;
    default:
      return range satisfies never;
  }
}

const PRESET_DAYS = { today: 1, "7d": 7, "30d": 30, "90d": 90 } as const;
const DAY_MS = 86_400_000;

/** `date` minus `days` calendar days (a UTC calendar date, so no zone is involved). */
function daysBefore(date: IsoDate, days: number): IsoDate {
  return new Date(Date.parse(`${date}T00:00:00Z`) - days * DAY_MS).toISOString().slice(0, 10);
}

/** The request as the backend takes it; `today` is the current day in the configured zone. */
export function toReportSpec(request: ReportRequest, today: IsoDate): ReportSpec {
  const range =
    request.range.kind === "custom"
      ? { first: request.range.from, last: request.range.to }
      : { first: daysBefore(today, PRESET_DAYS[request.range.preset] - 1), last: today };
  return {
    range: { kind: "custom", ...range },
    scope: request.scope,
    format: request.format,
    currency: request.currency,
  };
}
