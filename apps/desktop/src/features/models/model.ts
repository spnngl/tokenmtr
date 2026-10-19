import type { Currency } from "../../shared/format";
import type { TokenMix } from "../../shared/schemas";
import type { ModelRow } from "../../shared/schemas/usage";
import { seriesOf, TOP_MODELS, topModels } from "../../shared/top-models";

/** What the Models screen renders: rows in `currency`. */
export interface ModelsData {
  readonly currency: Currency;
  readonly rows: readonly ModelRow[];
}

/** Share of a model's priced-or-not events by cost source, as fractions of its events. */
export interface CostSourceMix {
  readonly reported: number;
  readonly estimated: number;
  readonly unpriced: number;
}

/** The cost-source mix of a row. An event-less row is all zeros. */
export function costSourceMix(row: ModelRow): CostSourceMix {
  const total = row.reported_events + row.estimated_events + row.unpriced_events;
  if (total === 0) {
    return { reported: 0, estimated: 0, unpriced: 0 };
  }
  return {
    reported: row.reported_events / total,
    estimated: row.estimated_events / total,
    unpriced: row.unpriced_events / total,
  };
}

/** Models with events no price covers: their API value is a lower bound. */
export function unpricedModels(rows: readonly ModelRow[]): readonly ModelRow[] {
  return rows.filter((row) => row.unpriced_events > 0);
}

/** Models whose reported and estimated costs differ by more than 5 % (`divergent`). */
export function divergentModels(rows: readonly ModelRow[]): readonly ModelRow[] {
  return rows.filter((row) => row.divergent);
}

/** Tokens and API value of one chart series: a model, or every model folded into "Other". */
export interface ModelTotals extends TokenMix {
  readonly model: string;
  readonly total_tokens: number;
  readonly api_value_micros: number;
  /** An estimate, missing price or old FX rate contributes. */
  readonly approximate: boolean;
}

/**
 * The series of a per-model chart: the top models by API value, then "Other" when more remain.
 * Every chart of a view shares them, so a model keeps its colour across charts.
 */
export function chartModels(
  rows: readonly Pick<ModelTotals, "model" | "api_value_micros">[],
): readonly string[] {
  return topModels(
    rows.map((row): readonly [string, number] => [row.model, row.api_value_micros]),
    TOP_MODELS,
  );
}

function addTotals(sum: ModelTotals, row: ModelTotals): ModelTotals {
  return {
    model: sum.model,
    input_uncached: sum.input_uncached + row.input_uncached,
    cache_read: sum.cache_read + row.cache_read,
    cache_write_5m: sum.cache_write_5m + row.cache_write_5m,
    cache_write_1h: sum.cache_write_1h + row.cache_write_1h,
    output: sum.output + row.output,
    reasoning: sum.reasoning + row.reasoning,
    total_tokens: sum.total_tokens + row.total_tokens,
    api_value_micros: sum.api_value_micros + row.api_value_micros,
    approximate: sum.approximate || row.approximate,
  };
}

const noTotals = (model: string): ModelTotals => ({
  model,
  input_uncached: 0,
  cache_read: 0,
  cache_write_5m: 0,
  cache_write_1h: 0,
  output: 0,
  reasoning: 0,
  total_tokens: 0,
  api_value_micros: 0,
  approximate: false,
});

/** `rows` summed per series of `top` (see {@link chartModels}), in `top` order. */
export function foldModels(
  rows: readonly ModelTotals[],
  top: readonly string[],
): readonly ModelTotals[] {
  const folded = new Map<string, ModelTotals>();
  for (const row of rows) {
    const model = seriesOf(row.model, top);
    folded.set(model, addTotals(folded.get(model) ?? noTotals(model), row));
  }
  return top.flatMap((model) => folded.get(model) ?? []);
}

/** API value per million tokens, in micros; `null` without tokens. */
export function costPerMillion(row: ModelTotals): number | null {
  return row.total_tokens === 0 ? null : (row.api_value_micros / row.total_tokens) * 1_000_000;
}
