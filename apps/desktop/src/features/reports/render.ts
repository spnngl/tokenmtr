import type { Currency } from "../../shared/format";
import { formatTokens } from "../../shared/format";
import type { ModelRow, ProjectRow, SeriesPoint, Totals } from "../../shared/schemas";
import { formatAmount } from "../common/amount";
import {
  type GeneratedReport,
  type ReportRequest,
  rangeLabel,
  reportFilename,
  scopeLabel,
} from "./request";

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
    formatAmount(line.apiValue, currency),
    formatAmount(line.paid, currency),
  ];
}

function summaryRows(
  request: ReportRequest,
  data: ReportData,
): readonly (readonly [string, string])[] {
  return [
    ["Period", rangeLabel(request.range)],
    ["Scope", scopeLabel(request.scope)],
    ["Sessions", data.totals.sessions.toLocaleString("en-US")],
    ["API value", formatAmount(data.totals.api_value_micros, data.currency)],
    ["Paid", formatAmount(data.totals.paid_micros, data.currency)],
    ["Tokens", formatTokens(data.totals.total_tokens)],
  ];
}

function markdownCell(text: string): string {
  return text.replace(/\|/g, "\\|").replace(/\n/g, " ");
}

function markdown(request: ReportRequest, data: ReportData): string {
  const out = ["# TokenMtr usage report", ""];
  for (const [label, value] of summaryRows(request, data)) {
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

function html(request: ReportRequest, data: ReportData): string {
  const out = [
    "<!doctype html>",
    '<html lang="en">',
    `<head><meta charset="utf-8"><title>TokenMtr usage report</title><style>${HTML_STYLE}</style></head>`,
    "<body>",
    "<h1>TokenMtr usage report</h1>",
    "<dl>",
  ];
  for (const [label, value] of summaryRows(request, data)) {
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

/** Renders `data` in the requested format; the later Rust renderer produces the same content. */
export function renderReport(request: ReportRequest, data: ReportData): GeneratedReport {
  const filename = reportFilename(request);
  const events = data.totals.events;
  switch (request.format) {
    case "markdown":
      return { format: "markdown", filename, events, content: markdown(request, data) };
    case "csv":
      return { format: "csv", filename, events, content: csv(data) };
    case "html":
      return { format: "html", filename, events, content: html(request, data) };
    default:
      return request.format satisfies never;
  }
}
