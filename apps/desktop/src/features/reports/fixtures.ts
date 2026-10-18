import type { ModelRow, ProjectRow, SeriesPoint } from "../../shared/schemas";
import { micros, usage } from "../common/fixtures";
import type { ReportData } from "./render";
import type { ReportRequest } from "./request";

export const REQUEST: ReportRequest = {
  range: { kind: "preset", preset: "30d" },
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

export const PROJECT: ProjectRow = {
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
export const REPORT_DATA: ReportData = {
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
