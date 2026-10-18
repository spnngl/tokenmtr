import { z } from "zod";
import { currencySchema, dateSchema } from "./primitives";

/**
 * Reports (D46): the argument of the Tauri commands `generate_report` and `save_report`
 * (`ReportWire` in `src-tauri/src/request.rs`) and the answer of `generate_report`.
 */

export const REPORT_FORMATS = ["markdown", "csv", "html"] as const;
const reportFormatSchema = z.enum(REPORT_FORMATS);
export type ReportFormat = z.infer<typeof reportFormatSchema>;

/** What a report covers: everything, one project or one client. */
const reportScopeSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("all") }),
  z.object({ kind: z.literal("project"), project: z.string() }),
  z.object({ kind: z.literal("client"), client: z.string() }),
]);
export type ReportScope = z.infer<typeof reportScopeSchema>;

/** `first` to `last`, both inclusive. The shell also accepts `{ kind: "month", date }`; unused here. */
const reportRangeSchema = z.object({
  kind: z.literal("custom"),
  first: dateSchema,
  last: dateSchema,
});

/** One report to generate or save. */
const reportSpecSchema = z.object({
  range: reportRangeSchema,
  scope: reportScopeSchema,
  format: reportFormatSchema,
  currency: currencySchema,
});
export type ReportSpec = z.infer<typeof reportSpecSchema>;

/** A rendered report: `content` is the file body, `filename` the suggested save name. */
export const generatedReportSchema = z.object({
  format: reportFormatSchema,
  filename: z.string(),
  content: z.string(),
});
export type GeneratedReport = z.infer<typeof generatedReportSchema>;
