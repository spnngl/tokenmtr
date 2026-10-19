/** Per-model charts show this many models; the rest fold into {@link OTHER_MODEL}. */
export const TOP_MODELS = 5;
/** The series of every model outside the top ones. */
export const OTHER_MODEL = "Other";
/** Label of tool calls and signals whose session has no usage event, hence no model. */
export const UNKNOWN_MODEL = "Unknown model";

/**
 * The `limit` models of highest total (ties by name), then {@link OTHER_MODEL} when any other model
 * remains. `totals` may repeat a model; its values add up.
 */
export function topModels(
  totals: Iterable<readonly [string, number]>,
  limit: number = TOP_MODELS,
): readonly string[] {
  const sums = new Map<string, number>();
  for (const [model, value] of totals) {
    sums.set(model, (sums.get(model) ?? 0) + value);
  }
  const ranked = [...sums]
    .toSorted(([a, x], [b, y]) => y - x || a.localeCompare(b))
    .map(([model]) => model);
  return ranked.length <= limit ? ranked : [...ranked.slice(0, limit), OTHER_MODEL];
}

/** The series `model` counts under: itself when among `top`, else {@link OTHER_MODEL}. */
export function seriesOf(model: string, top: readonly string[]): string {
  return top.includes(model) ? model : OTHER_MODEL;
}

/**
 * Sums `items` into one value per `top` series (in `top` order) for each row key, rows in first-seen
 * order.
 */
export function splitRows<T>(
  items: Iterable<T>,
  top: readonly string[],
  key: (item: T) => { readonly row: string; readonly model: string; readonly value: number },
): Map<string, number[]> {
  const rows = new Map<string, number[]>();
  for (const item of items) {
    const { row, model, value } = key(item);
    const values = rows.get(row) ?? top.map(() => 0);
    const index = top.indexOf(seriesOf(model, top));
    if (index >= 0) {
      values[index] = (values[index] ?? 0) + value;
    }
    rows.set(row, values);
  }
  return rows;
}
