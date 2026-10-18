import type { DataSource } from "../../shared/data-source";
import { type ReportData, renderReport } from "./render";
import { type GenerateReport, requestFilters } from "./request";

/**
 * Renders reports from the query commands. It stands in for the Tauri command `generate_report`
 * (`tokenmtr-report`, D46) and is replaced by it without touching the screen.
 */
export function localGenerateReport(
  source: Pick<DataSource, "overview" | "projects" | "models" | "series">,
): GenerateReport {
  return async (request) => {
    const filters = requestFilters(request);
    const [totals, projects, models, series] = await Promise.all([
      source.overview(filters),
      source.projects(filters),
      source.models(filters),
      source.series(filters, "none"),
    ]);
    const data: ReportData = {
      currency: totals.currency,
      totals,
      projects,
      models,
      days: series.points,
    };
    return renderReport(request, data);
  };
}
