import { afterEach, describe, expect, it } from "vitest";
import { printHtml } from "./save";

const REPORT = { format: "html", filename: "report.html", content: "<p>Report</p>" } as const;

function frames(): HTMLIFrameElement[] {
  return [...document.querySelectorAll("iframe")];
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("printHtml", () => {
  it("sandboxes the report: no script runs", () => {
    printHtml(REPORT);
    const [frame] = frames();
    expect(frame?.getAttribute("sandbox")).toBe("allow-same-origin allow-modals");
    expect(frame?.srcdoc).toBe("<p>Report</p>");
  });

  it("keeps one frame at a time", () => {
    printHtml(REPORT);
    printHtml(REPORT);
    expect(frames()).toHaveLength(1);
  });
});
