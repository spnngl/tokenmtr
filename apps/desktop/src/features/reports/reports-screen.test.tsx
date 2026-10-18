import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";
import type { Filters } from "../../shared/filters";

import type { Remote } from "../common/remote-view";
import { PROJECT, REPORT_DATA, REQUEST } from "./fixtures";
import { localGenerateReport } from "./local-report";
import { renderReport } from "./render";
import { ReportsScreen } from "./reports-screen";
import { type GeneratedReport, type ReportRequest, reportFilename } from "./request";

afterEach(cleanup);

const OPTIONS = { projects: ["acme/web", "acme/api"], clients: ["Acme"] };

describe("renderReport", () => {
  it("renders totals, per project, per model and per day in Markdown", () => {
    const { content, events } = renderReport(REQUEST, REPORT_DATA);
    expect(events).toBe(2000);
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
    const { content } = renderReport({ ...REQUEST, format: "csv" }, data);
    const lines = content.trim().split("\n");
    expect(lines[0]).toBe("section,name,events,total_tokens,api_value_usd,paid_usd");
    expect(lines[1]).toBe("total,Total,2000,20000000,200.00,80.00");
    expect(content).toContain('"\'=HYPERLINK(""x""),y"');
    expect(lines).toHaveLength(1 + 1 + 1 + 1 + 20);
  });

  it("escapes HTML and notes approximate EUR figures", () => {
    const data = { ...REPORT_DATA, currency: "EUR" as const };
    const { content } = renderReport({ ...REQUEST, format: "html", currency: "EUR" }, data);
    expect(content).toContain("<!doctype html>");
    expect(content).toContain("Web | storefront");
    expect(content).toContain("@media print");
    expect(content).toContain("approximate");
    const hostile = {
      ...REPORT_DATA,
      projects: [{ ...PROJECT, name: "<script>alert(1)</script>" }],
    };
    expect(renderReport({ ...REQUEST, format: "html" }, hostile).content).not.toContain("<script>");
  });

  it("names the file after scope, range and format", () => {
    expect(reportFilename(REQUEST)).toBe("tokenmtr-report-all-30d.md");
    expect(
      reportFilename({
        ...REQUEST,
        scope: { kind: "project", project: "acme/web" },
        format: "csv",
      }),
    ).toBe("tokenmtr-report-project-acme-web-30d.csv");
  });
});

describe("localGenerateReport", () => {
  it("queries with the request's scope and renders the result", async () => {
    const overview = vi.fn((_filters: Filters) => Promise.resolve(REPORT_DATA.totals));
    const generate = localGenerateReport({
      overview,
      projects: () => Promise.resolve([...REPORT_DATA.projects]),
      models: () => Promise.resolve([...REPORT_DATA.models]),
      series: () =>
        Promise.resolve({
          currency: "USD",
          bucket: "day",
          group_by: "none",
          points: [...REPORT_DATA.days],
        }),
    });
    const report = await generate({ ...REQUEST, scope: { kind: "client", client: "Acme" } });
    expect(overview.mock.calls[0]?.[0].client).toBe("Acme");
    expect(overview.mock.calls[0]?.[0].project).toBeNull();
    expect(report.content).toContain("Scope:** Client Acme");
  });
});

function Harness({
  preview,
  onSave = vi.fn(),
  onPrint = vi.fn(),
  initial = REQUEST,
}: {
  readonly preview: (request: ReportRequest) => Remote<GeneratedReport>;
  readonly onSave?: (report: GeneratedReport) => void;
  readonly onPrint?: (request: ReportRequest) => void;
  readonly initial?: ReportRequest;
}) {
  const [request, setRequest] = useState(initial);
  return (
    <ReportsScreen
      request={request}
      options={OPTIONS}
      onRequestChange={setRequest}
      preview={preview(request)}
      onSave={onSave}
      onPrint={onPrint}
    />
  );
}

const ready = (request: ReportRequest): Remote<GeneratedReport> => ({
  kind: "ready",
  data: renderReport(request, REPORT_DATA),
});

describe("ReportsScreen", () => {
  it("loading", () => {
    render(<Harness preview={() => ({ kind: "loading" })} />);
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
  });

  it("empty when the period holds no usage", () => {
    render(
      <Harness
        preview={(request) => ({
          kind: "ready",
          data: { ...renderReport(request, REPORT_DATA), events: 0 },
        })}
      />,
    );
    expect(screen.getByText("No usage in this period")).toBeTruthy();
  });

  it("error retries", async () => {
    const onRetry = vi.fn();
    render(<Harness preview={() => ({ kind: "error", message: "Store is locked", onRetry })} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("daemon-offline offers to start it", () => {
    render(<Harness preview={() => ({ kind: "daemon-offline", onStartDaemon: vi.fn() })} />);
    expect(screen.getByRole("button", { name: "Install / start daemon" })).toBeTruthy();
  });

  it("previews, switches format, saves and prints", async () => {
    const onSave = vi.fn();
    const onPrint = vi.fn();
    const { container } = render(<Harness preview={ready} onSave={onSave} onPrint={onPrint} />);
    const user = userEvent.setup();

    expect(screen.getByLabelText("Report preview").textContent).toContain(
      "# TokenMtr usage report",
    );
    expect(await axeViolations(container)).toEqual([]);

    await user.click(screen.getByRole("radio", { name: "CSV" }));
    expect(screen.getByLabelText("Report preview").textContent).toContain("section,name,events");
    await user.click(screen.getByRole("radio", { name: "HTML" }));
    const frame = screen.getByTitle("Report preview");
    expect(frame.getAttribute("sandbox")).toBe("");
    // axe cannot message frames in jsdom; the frame's content is the escaped report checked above.
    frame.remove();
    expect(await axeViolations(container)).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Save HTML" }));
    expect(onSave.mock.calls[0]?.[0].filename).toBe("tokenmtr-report-all-30d.html");
    await user.click(screen.getByRole("button", { name: "Save as PDF" }));
    expect(onPrint.mock.calls[0]?.[0].format).toBe("html");
  });

  it("offers project scope and reaches every control by keyboard", async () => {
    render(<Harness preview={ready} />);
    const user = userEvent.setup();

    await user.tab();
    expect(document.activeElement?.textContent).toContain("Last 30 days");
    await user.tab();
    expect(document.activeElement?.textContent).toContain("Everything");

    await user.click(screen.getByRole("button", { name: /Scope/ }));
    await user.click(await screen.findByRole("option", { name: "One project" }));
    expect(screen.getByText("Project acme/web", { exact: false })).toBeTruthy();
  });
});
