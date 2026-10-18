/**
 * Chart point budget. `tokenmtr-query` already picks a bucket size that keeps a series
 * under this many points; the helpers here are the second line of defence for custom ranges and
 * for lists (tools, branches) that have no natural bucket.
 */
const MAX_CHART_POINTS = 400;

/**
 * Merges consecutive rows until at most `max` remain. `combine` must be associative (sums,
 * maxima); the merged row keeps the first row's identity (its date or label).
 */
export function mergeBuckets<T>(
  rows: readonly T[],
  combine: (first: T, next: T) => T,
  max: number = MAX_CHART_POINTS,
): readonly T[] {
  if (max < 1) {
    throw new RangeError("max must be at least 1");
  }
  if (rows.length <= max) {
    return rows;
  }
  const size = Math.ceil(rows.length / max);
  const merged: T[] = [];
  for (let start = 0; start < rows.length; start += size) {
    const [first, ...rest] = rows.slice(start, start + size);
    if (first !== undefined) {
      merged.push(rest.reduce(combine, first));
    }
  }
  return merged;
}

/** The first `limit` rows of an already sorted list, for bar charts. The table keeps them all. */
export function topRows<T>(rows: readonly T[], limit: number): readonly T[] {
  return rows.length <= limit ? rows : rows.slice(0, limit);
}
