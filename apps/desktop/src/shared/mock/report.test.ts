import { describe, expect, it } from "vitest";
import { micros, usage } from "../fixtures";
import type { ModelRow, ProjectRow, ReportSpec, SeriesPoint } from "../schemas";
import { generateMockReport, type ReportData, renderReport } from "./report";

const SPEC: ReportSpec = {
  range: { kind: "custom", first: "2026-05-01", last: "2026-05-30" },
  scope: { kind: "all" },
  format: "markdown",
  currency: "USD",
};

function day(index: number): SeriesPoint {
  const date = new Date(Date.UTC(2026, 4, 1 + index)).toISOString().slice(0, 10);
  return {
    start: date,
    minutes: 60,
    ...usage({
      events: 100,
      total_tokens: 1_000_000,
      api_value_micros: micros(10),
      paid_micros: micros(4),
    }),
    cumulative_api_value_micros: micros(10 * (index + 1)),
    cumulative_paid_micros: micros(4 * (index + 1)),
    groups: {},
  };
}

const PROJECT: ProjectRow = {
  project: "acme/web",
  name: "Web | storefront",
  client: "Acme",
  ...usage({
    events: 2000,
    total_tokens: 20_000_000,
    api_value_micros: micros(200),
    paid_micros: micros(80),
  }),
  share_percent: 100,
};

const MODEL: ModelRow = {
  model: "claude-sonnet-4",
  ...usage({
    events: 2000,
    total_tokens: 20_000_000,
    api_value_micros: micros(200),
    paid_micros: micros(80),
  }),
  share_percent: 100,
  reported_vs_estimated_percent: null,
  divergent: false,
};

/** Twenty daily points, one project whose name needs escaping, one model. */
const REPORT_DATA: ReportData = {
  currency: "USD",
  totals: {
    currency: "USD",
    currency_fallback: false,
    ...usage({
      events: 2000,
      total_tokens: 20_000_000,
      api_value_micros: micros(200),
      paid_micros: micros(80),
    }),
    cached_input_percent: 60,
    cache_savings_micros: null,
    roi: null,
    sessions: 42,
  },
  projects: [PROJECT],
  models: [MODEL],
  days: Array.from({ length: 20 }, (_, index) => day(index)),
};

describe("renderReport", () => {
  it("renders totals, per project, per model and per day in Markdown", () => {
    const { content } = renderReport(SPEC, REPORT_DATA);
    expect(content).toContain("- **Sessions:** 42");
    expect(content).toContain("- **API value:** $200.00");
    expect(content).toContain("## Per project");
    expect(content).toContain("Web \\| storefront");
    expect(content).toContain("## Per model");
    expect(content).toContain("## Per day");
    expect(content).toContain("2026-05-20");
  });

  it("renders CSV with one header and neutralised formulas", () => {
    const data = {
      ...REPORT_DATA,
      projects: [{ ...PROJECT, name: '=HYPERLINK("x"),y' }],
    };
    const { content } = renderReport({ ...SPEC, format: "csv" }, data);
    const lines = content.trim().split("\n");
    expect(lines[0]).toBe("section,name,events,total_tokens,api_value_usd,paid_usd");
    expect(lines[1]).toBe("total,Total,2000,20000000,200.00,80.00");
    expect(content).toContain('"\'=HYPERLINK(""x""),y"');
    expect(lines).toHaveLength(1 + 1 + 1 + 1 + 20);
  });

  it("escapes HTML and notes approximate EUR figures", () => {
    const data = { ...REPORT_DATA, currency: "EUR" as const };
    const { content } = renderReport({ ...SPEC, format: "html", currency: "EUR" }, data);
    expect(content).toContain("<!doctype html>");
    expect(content).toContain("Web | storefront");
    expect(content).toContain("@media print");
    expect(content).toContain("approximate");
    const hostile = {
      ...REPORT_DATA,
      projects: [{ ...PROJECT, name: "<script>alert(1)</script>" }],
    };
    expect(renderReport({ ...SPEC, format: "html" }, hostile).content).not.toContain("<script>");
  });
});

describe("generateMockReport", () => {
  it("names the file after range, scope and format", () => {
    expect(generateMockReport(SPEC, false).filename).toBe(
      "tokenmtr-report-2026-05-01_2026-05-30.md",
    );
    const scoped = generateMockReport(
      { ...SPEC, scope: { kind: "project", project: "acme/web" }, format: "csv" },
      false,
    );
    expect(scoped.filename).toBe("tokenmtr-report-2026-05-01_2026-05-30-project-acme-web.csv");
  });

  it("scopes the content and renders a report even for an empty store", () => {
    const scoped = generateMockReport(
      { ...SPEC, scope: { kind: "client", client: "Acme" } },
      false,
    );
    expect(scoped.content).toContain("Scope:** Client Acme");
    expect(generateMockReport(SPEC, true).content).toContain("## Per day");
  });
});
