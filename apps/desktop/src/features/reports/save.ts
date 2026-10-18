import type { GeneratedReport } from "../../shared/schemas";

/** Opens the webview print dialog on an HTML report: "Save as PDF" (D46). */
export function printHtml(report: GeneratedReport): void {
  const frame = document.createElement("iframe");
  frame.title = "Report to print";
  frame.hidden = true;
  frame.srcdoc = report.content;
  frame.addEventListener("load", () => {
    frame.contentWindow?.print();
    // Keep the frame until the dialog closes; the next print replaces it.
    setTimeout(() => frame.remove(), 60_000);
  });
  document.body.append(frame);
}
