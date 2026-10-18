import type { GeneratedReport } from "./request";

const MIME = {
  markdown: "text/markdown",
  csv: "text/csv",
  html: "text/html",
} as const satisfies Record<GeneratedReport["format"], string>;

/** Saves the report through the webview's download. The Tauri file dialog replaces it later. */
export function downloadReport(report: GeneratedReport): void {
  const url = URL.createObjectURL(
    new Blob([report.content], { type: `${MIME[report.format]};charset=utf-8` }),
  );
  const link = document.createElement("a");
  link.href = url;
  link.download = report.filename;
  document.body.append(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

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
