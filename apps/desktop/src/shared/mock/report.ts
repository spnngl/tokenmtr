import type { Filters } from "../filters";
import type { Currency } from "../format";
import { formatCurrencyMicros, formatTokens } from "../format";
import type {
  GeneratedReport,
  ModelRow,
  ProjectRow,
  ReportScope,
  ReportSpec,
  SeriesPoint,
  Totals,
} from "../schemas";
import { rangeOf } from "./aggregate";
import { factsFor, modelsOf, projectsOf, seriesOf, totalsOf } from "./reports";
import { sessionsOf } from "./sessions";

function rangeLabel(range: ReportSpec["range"]): string {
  return `${range.first} to ${range.last}`;
}

function scopeLabel(scope: ReportScope): string {
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

const EXTENSIONS = { markdown: "md", csv: "csv", html: "html" } as const satisfies Record<
  ReportSpec["format"],
  string
>;

function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function reportFilename(spec: ReportSpec): string {
  const scope = spec.scope.kind === "all" ? "" : `-${slug(scopeLabel(spec.scope))}`;
  return `tokenmtr-report-${spec.range.first}_${spec.range.last}${scope}.${EXTENSIONS[spec.format]}`;
}

/** The query results a report is made of. Money is already in `currency`. */
export interface ReportData {
  readonly currency: Currency;
  readonly totals: Totals;
  readonly projects: readonly ProjectRow[];
  readonly models: readonly ModelRow[];
  /** Daily points, oldest first. */
  readonly days: readonly SeriesPoint[];
}

/** One line of a per-project, per-model or per-day table. */
interface Line {
  readonly name: string;
  readonly events: number;
  readonly tokens: number;
  readonly apiValue: number;
  readonly paid: number;
}

const UNASSIGNED = "(unassigned)";

function projectLines(data: ReportData): readonly Line[] {
  return data.projects.map((row) => ({
    name: row.name ?? row.project ?? UNASSIGNED,
    events: row.events,
    tokens: row.total_tokens,
    apiValue: row.api_value_micros,
    paid: row.paid_micros,
  }));
}

function modelLines(data: ReportData): readonly Line[] {
  return data.models.map((row) => ({
    name: row.model,
    events: row.events,
    tokens: row.total_tokens,
    apiValue: row.api_value_micros,
    paid: row.paid_micros,
  }));
}

function dayLines(data: ReportData): readonly Line[] {
  return data.days.map((point) => ({
    name: point.start,
    events: point.events,
    tokens: point.total_tokens,
    apiValue: point.api_value_micros,
    paid: point.paid_micros,
  }));
}

function totalLine(data: ReportData): Line {
  return {
    name: "Total",
    events: data.totals.events,
    tokens: data.totals.total_tokens,
    apiValue: data.totals.api_value_micros,
    paid: data.totals.paid_micros,
  };
}

function approximateNote(data: ReportData): string | null {
  return data.currency === "EUR" || data.totals.approximate
    ? "Some figures are approximate: EUR is converted at the rate of each event's day, and some costs are estimated."
    : null;
}

const SECTIONS = [
  { title: "Per project", lines: projectLines },
  { title: "Per model", lines: modelLines },
  { title: "Per day", lines: dayLines },
] as const;

function cells(line: Line, currency: Currency): readonly string[] {
  return [
    line.name,
    line.events.toLocaleString("en-US"),
    formatTokens(line.tokens),
    formatCurrencyMicros(line.apiValue, currency),
    formatCurrencyMicros(line.paid, currency),
  ];
}

function summaryRows(spec: ReportSpec, data: ReportData): readonly (readonly [string, string])[] {
  return [
    ["Period", rangeLabel(spec.range)],
    ["Scope", scopeLabel(spec.scope)],
    ["Sessions", data.totals.sessions.toLocaleString("en-US")],
    ["API value", formatCurrencyMicros(data.totals.api_value_micros, data.currency)],
    ["Paid", formatCurrencyMicros(data.totals.paid_micros, data.currency)],
    ["Tokens", formatTokens(data.totals.total_tokens)],
  ];
}

function markdownCell(text: string): string {
  return text.replace(/\|/g, "\\|").replace(/\n/g, " ");
}

function markdown(spec: ReportSpec, data: ReportData): string {
  const out = ["# TokenMtr usage report", ""];
  for (const [label, value] of summaryRows(spec, data)) {
    out.push(`- **${label}:** ${markdownCell(value)}`);
  }
  const note = approximateNote(data);
  if (note !== null) {
    out.push("", `_${note}_`);
  }
  for (const section of SECTIONS) {
    out.push("", `## ${section.title}`, "");
    out.push(
      "| Name | Events | Tokens | API value | Paid |",
      "| --- | ---: | ---: | ---: | ---: |",
    );
    for (const line of [...section.lines(data), totalLine(data)]) {
      out.push(`| ${cells(line, data.currency).map(markdownCell).join(" | ")} |`);
    }
  }
  return `${out.join("\n")}\n`;
}

/** Quotes a CSV field; a leading formula character is neutralised for spreadsheet apps. */
function csvField(text: string): string {
  const safe = /^[=+\-@\t\r]/.test(text) ? `'${text}` : text;
  return /[",\n\r]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
}

function decimal(micros: number): string {
  return (micros / 1_000_000).toFixed(2);
}

function csv(data: ReportData): string {
  const unit = data.currency.toLowerCase();
  const rows: string[][] = [
    ["section", "name", "events", "total_tokens", `api_value_${unit}`, `paid_${unit}`],
  ];
  const push = (section: string, line: Line): void => {
    rows.push([
      section,
      line.name,
      String(line.events),
      String(line.tokens),
      decimal(line.apiValue),
      decimal(line.paid),
    ]);
  };
  push("total", totalLine(data));
  for (const [section, lines] of [
    ["project", projectLines(data)],
    ["model", modelLines(data)],
    ["day", dayLines(data)],
  ] as const) {
    for (const line of lines) {
      push(section, line);
    }
  }
  return `${rows.map((row) => row.map(csvField).join(",")).join("\n")}\n`;
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

const HTML_STYLE =
  "body{font:14px/1.5 system-ui,sans-serif;max-width:56rem;margin:2rem auto;padding:0 1rem;color:#111}" +
  "table{border-collapse:collapse;width:100%;margin-bottom:1.5rem}" +
  "th,td{border-bottom:1px solid #ccc;padding:.25rem .5rem;text-align:left}" +
  "td:not(:first-child),th:not(:first-child){text-align:right}" +
  "@media print{body{margin:0}h2{break-after:avoid}tr{break-inside:avoid}}";

function html(spec: ReportSpec, data: ReportData): string {
  const out = [
    "<!doctype html>",
    '<html lang="en">',
    `<head><meta charset="utf-8"><title>TokenMtr usage report</title><style>${HTML_STYLE}</style></head>`,
    "<body>",
    "<h1>TokenMtr usage report</h1>",
    "<dl>",
  ];
  for (const [label, value] of summaryRows(spec, data)) {
    out.push(`<dt>${escapeHtml(label)}</dt><dd>${escapeHtml(value)}</dd>`);
  }
  out.push("</dl>");
  const note = approximateNote(data);
  if (note !== null) {
    out.push(`<p><em>${escapeHtml(note)}</em></p>`);
  }
  for (const section of SECTIONS) {
    out.push(`<h2>${section.title}</h2>`, "<table>");
    out.push(
      "<thead><tr><th>Name</th><th>Events</th><th>Tokens</th><th>API value</th><th>Paid</th></tr></thead>",
      "<tbody>",
    );
    for (const line of [...section.lines(data), totalLine(data)]) {
      const row = cells(line, data.currency)
        .map((cell) => `<td>${escapeHtml(cell)}</td>`)
        .join("");
      out.push(`<tr>${row}</tr>`);
    }
    out.push("</tbody>", "</table>");
  }
  out.push("</body>", "</html>");
  return `${out.join("\n")}\n`;
}

/** Renders `data` in the format of `spec`; the Rust renderer (`tokenmtr-report`) produces the same sections. */
export function renderReport(spec: ReportSpec, data: ReportData): GeneratedReport {
  const filename = reportFilename(spec);
  switch (spec.format) {
    case "markdown":
      return { format: "markdown", filename, content: markdown(spec, data) };
    case "csv":
      return { format: "csv", filename, content: csv(data) };
    case "html":
      return { format: "html", filename, content: html(spec, data) };
    default:
      return spec.format satisfies never;
  }
}

/** The mock `generate_report`: aggregates the seeded facts the way the query commands do. */
export function generateMockReport(spec: ReportSpec, empty: boolean): GeneratedReport {
  const filters: Filters = {
    range: { kind: "custom", from: spec.range.first, to: spec.range.last },
    agent: null,
    profile: null,
    model: null,
    project: spec.scope.kind === "project" ? spec.scope.project : null,
    client: spec.scope.kind === "client" ? spec.scope.client : null,
    billing: "all",
    currency: spec.currency,
  };
  const facts = empty ? [] : factsFor(filters);
  const sessions = empty ? 0 : sessionsOf(filters, 0, 1).total;
  return renderReport(spec, {
    currency: spec.currency,
    totals: totalsOf(facts, spec.currency, sessions),
    projects: projectsOf(facts, spec.currency),
    models: modelsOf(facts, spec.currency),
    days: seriesOf(facts, rangeOf(filters.range), spec.currency, "none").points,
  });
}
