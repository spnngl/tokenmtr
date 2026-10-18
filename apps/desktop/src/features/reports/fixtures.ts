import type { GeneratedReport } from "../../shared/schemas";
import type { ReportRequest } from "./request";

export const REQUEST: ReportRequest = {
  range: { kind: "preset", preset: "30d" },
  scope: { kind: "all" },
  format: "markdown",
  currency: "USD",
};

const BODIES = {
  markdown: "# TokenMtr usage report\n\n- **Sessions:** 42\n",
  csv: "section,name,events,total_tokens,api_value_usd,paid_usd\ntotal,Total,2000,20000000,200.00,80.00\n",
  html: "<!doctype html><html><body><h1>TokenMtr usage report</h1></body></html>",
} as const;

/** What `generate_report` answers for `request`. */
export function generated(request: ReportRequest): GeneratedReport {
  const extension = { markdown: "md", csv: "csv", html: "html" }[request.format];
  return {
    format: request.format,
    filename: `tokenmtr-report-2026-05.${extension}`,
    content: BODIES[request.format],
  };
}
