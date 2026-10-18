import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";

import type { Remote } from "../../shared/remote-view";
import type { GeneratedReport } from "../../shared/schemas";
import { generated, REQUEST } from "./fixtures";
import { ReportsScreen, type SaveStatus } from "./reports-screen";
import { type ReportRequest, toReportSpec } from "./request";

afterEach(cleanup);

const OPTIONS = { projects: ["acme/web", "acme/api"], clients: ["Acme"] };

describe("toReportSpec", () => {
  it("pins presets to the days ending today", () => {
    expect(toReportSpec(REQUEST, "2026-05-30").range).toEqual({
      kind: "custom",
      first: "2026-05-01",
      last: "2026-05-30",
    });
    expect(
      toReportSpec({ ...REQUEST, range: { kind: "preset", preset: "today" } }, "2026-05-30").range,
    ).toEqual({ kind: "custom", first: "2026-05-30", last: "2026-05-30" });
  });

  it("keeps a custom range, scope, format and currency", () => {
    const spec = toReportSpec(
      {
        range: { kind: "custom", from: "2026-04-01", to: "2026-04-15" },
        scope: { kind: "client", client: "Acme" },
        format: "csv",
        currency: "EUR",
      },
      "2026-05-30",
    );
    expect(spec).toEqual({
      range: { kind: "custom", first: "2026-04-01", last: "2026-04-15" },
      scope: { kind: "client", client: "Acme" },
      format: "csv",
      currency: "EUR",
    });
  });
});

const IDLE = { kind: "idle" } as const;

function Harness({
  preview,
  onSave = vi.fn(),
  onPrint = vi.fn(),
  saveStatus = IDLE,
  initial = REQUEST,
}: {
  readonly preview: (request: ReportRequest) => Remote<GeneratedReport>;
  readonly onSave?: () => void;
  readonly onPrint?: () => void;
  readonly saveStatus?: SaveStatus;
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
      saveStatus={saveStatus}
      onPrint={onPrint}
      isPrinting={false}
    />
  );
}

const ready = (request: ReportRequest): Remote<GeneratedReport> => ({
  kind: "ready",
  data: generated(request),
});

describe("ReportsScreen", () => {
  it("loading", () => {
    render(<Harness preview={() => ({ kind: "loading" })} />);
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
  });

  it("error retries", async () => {
    const onRetry = vi.fn();
    render(
      <Harness
        preview={() => ({
          kind: "error",
          title: "Request not valid",
          message: "Store is locked",
          onRetry,
        })}
      />,
    );
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
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
    expect(onSave).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: "Save as PDF" }));
    expect(onPrint).toHaveBeenCalledOnce();
  });

  it("shows where the report was saved, or why it was not", () => {
    const { rerender } = render(
      <Harness preview={ready} saveStatus={{ kind: "saved", path: "/tmp/report.md" }} />,
    );
    expect(screen.getByRole("status").textContent).toBe("Saved to /tmp/report.md");
    rerender(<Harness preview={ready} saveStatus={{ kind: "error", message: "disk full" }} />);
    expect(screen.getByRole("alert").textContent).toContain("disk full");
  });

  it("marks Save as pending while saving, keeping its label", () => {
    render(<Harness preview={ready} saveStatus={{ kind: "saving" }} />);
    const save = screen.getByRole("button", { name: "Save Markdown" });
    expect(save.getAttribute("aria-disabled")).toBe("true");
    expect(save.hasAttribute("data-pending")).toBe(true);
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
    expect(screen.getAllByText("acme/web").length).toBeGreaterThan(0);
  });
});
