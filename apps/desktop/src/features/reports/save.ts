import type { GeneratedReport } from "../../shared/schemas";

const FRAME_ID = "report-print-frame";

/**
 * Opens the webview print dialog on an HTML report: "Save as PDF" (D46). One frame at a time: a
 * new print replaces the previous frame, and the frame goes once the dialog closes.
 */
export function printHtml(report: GeneratedReport): void {
  document.getElementById(FRAME_ID)?.remove();
  const frame = document.createElement("iframe");
  frame.id = FRAME_ID;
  frame.title = "Report to print";
  frame.hidden = true;
  // No script of the report runs; same origin lets this window call `print()`, which needs modals.
  frame.setAttribute("sandbox", "allow-same-origin allow-modals");
  frame.srcdoc = report.content;
  frame.addEventListener("load", () => {
    const view = frame.contentWindow;
    if (view === null) {
      return;
    }
    view.addEventListener("afterprint", () => frame.remove(), { once: true });
    view.print();
  });
  document.body.append(frame);
}
