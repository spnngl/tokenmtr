import type { Filters, Range } from "../../shared/filters";
import type { Currency } from "../../shared/format";

export const REPORT_FORMATS = ["markdown", "csv", "html"] as const;
export type ReportFormat = (typeof REPORT_FORMATS)[number];

/** What a report covers: everything, one project or one client (D46). */
export type ReportScope =
  | { readonly kind: "all" }
  | { readonly kind: "project"; readonly project: string }
  | { readonly kind: "client"; readonly client: string };

/**
 * One report to render and save. It is the contract of the later Tauri command `generate_report`
 * (`tokenmtr-report`); until then `local-report.ts` renders the same content from queries.
 */
export interface ReportRequest {
  readonly range: Range;
  readonly scope: ReportScope;
  readonly format: ReportFormat;
  readonly currency: Currency;
}

/** A rendered report: `content` is the file body, `filename` the suggested save name. */
export interface GeneratedReport {
  readonly format: ReportFormat;
  readonly filename: string;
  readonly content: string;
  /** Usage events the report covers; 0 means the period holds no usage. */
  readonly events: number;
}

/** Renders a report; rejects with an `Error` whose message is shown to the user. */
export type GenerateReport = (request: ReportRequest) => Promise<GeneratedReport>;

const EXTENSIONS = { markdown: "md", csv: "csv", html: "html" } as const satisfies Record<
  ReportFormat,
  string
>;

export function formatExtension(format: ReportFormat): string {
  return EXTENSIONS[format];
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

/** The query filters a request stands for. */
export function requestFilters(request: ReportRequest): Filters {
  return {
    range: request.range,
    agent: null,
    profile: null,
    model: null,
    project: request.scope.kind === "project" ? request.scope.project : null,
    client: request.scope.kind === "client" ? request.scope.client : null,
    billing: "all",
    currency: request.currency,
  };
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

export function scopeLabel(scope: ReportScope): string {
  switch (scope.kind) {
    case "all":
      return "Everything";
    case "project":
      return `Project ${scope.project}`;
    case "client":
      return `Client ${scope.client}`;
    default:
      return scope satisfies never;
  }
}

function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function reportFilename(request: ReportRequest): string {
  const scope = request.scope.kind === "all" ? "all" : slug(scopeLabel(request.scope));
  const range =
    request.range.kind === "preset"
      ? request.range.preset
      : `${request.range.from}_${request.range.to}`;
  return `tokenmtr-report-${scope}-${range}.${formatExtension(request.format)}`;
}
