import type { Currency } from "../../shared/format";
import type { ModelRow } from "../../shared/schemas/usage";

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
