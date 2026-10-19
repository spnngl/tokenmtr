import type { BarDatum } from "../../shared/bar-chart";
import type { ColumnDatum } from "../../shared/column-chart";
import { type Currency, formatCurrencyMicros } from "../../shared/format";
import type { CostBucket, SessionAnalytics, SessionPoint, TokenMix } from "../../shared/schemas";
import { splitRows, topModels } from "../../shared/top-models";

/** Short, recognisable form of a session id (the tables print the same prefix). */
export function shortSession(session: string): string {
  return session.slice(0, 8);
}

/** Axis label of a cost bucket: "< $0.01" for the first, "≥ $100.00" for the open last one. */
export function costBucketLabel(bucket: CostBucket, currency: Currency): string {
  if (bucket.max_micros === null) {
    return `≥ ${formatCurrencyMicros(bucket.min_micros, currency)}`;
  }
  if (bucket.min_micros === 0) {
    return `< ${formatCurrencyMicros(bucket.max_micros, currency)}`;
  }
  return `${formatCurrencyMicros(bucket.min_micros, currency)}–${formatCurrencyMicros(bucket.max_micros, currency)}`;
}

/**
 * The cost histogram stacked by dominant model: the top models by session count plus "Other",
 * one row per bucket (empty buckets included) with its sessions per series.
 */
export function histogramByModel(data: SessionAnalytics): {
  readonly series: readonly string[];
  readonly rows: readonly BarDatum[];
} {
  const series = topModels(
    data.histogram.flatMap((bucket) => bucket.models.map((m) => [m.model, m.sessions] as const)),
  );
  const rows = data.histogram.map((bucket): BarDatum => {
    const label = costBucketLabel(bucket, data.currency);
    const split = splitRows(bucket.models, series, (m) => ({
      row: label,
      model: m.model,
      value: m.sessions,
    }));
    return { label, values: split.get(label) ?? series.map(() => 0) };
  });
  return { series, rows };
}

const CACHE_BINS = 10;

/**
 * Sessions per 10-point cache-hit band ("0–10%" .. "90–100%", 100 in the last); sessions without
 * input have no ratio and are left out.
 */
export function cacheHitBins(rows: readonly SessionPoint[]): ColumnDatum[] {
  const counts = new Array<number>(CACHE_BINS).fill(0);
  for (const row of rows) {
    if (row.cache_hit_percent !== null) {
      const bin = Math.min(CACHE_BINS - 1, Math.floor(row.cache_hit_percent / CACHE_BINS));
      counts[bin] = (counts[bin] ?? 0) + 1;
    }
  }
  return counts.map((value, index) => ({
    label: `${index * CACHE_BINS}–${(index + 1) * CACHE_BINS}%`,
    value,
  }));
}

/** A session with failed tool calls and its failure rate (0-1). */
export interface ToolErrorRank {
  readonly row: SessionPoint;
  readonly rate: number;
}

/** Sessions with at least one failed tool call, highest failure rate first (then most failures). */
export function toolErrorRanking(rows: readonly SessionPoint[], limit: number): ToolErrorRank[] {
  return rows
    .filter((row) => row.tool_errors > 0 && row.tool_calls > 0)
    .map((row) => ({ row, rate: row.tool_errors / row.tool_calls }))
    .sort((a, b) => b.rate - a.rate || b.row.tool_errors - a.row.tool_errors)
    .slice(0, limit);
}

/** Series of a token mix bar, in stack order. Output excludes reasoning, which has its own. */
export const TOKEN_MIX_SERIES = [
  { name: "Input" },
  { name: "Cache read" },
  { name: "Cache write" },
  { name: "Output" },
  { name: "Reasoning" },
] as const;

/** The disjoint values of `TOKEN_MIX_SERIES` (cache writes summed, reasoning out of output). */
export function tokenMixValues(mix: TokenMix): number[] {
  return [
    mix.input_uncached,
    mix.cache_read,
    mix.cache_write_5m + mix.cache_write_1h,
    Math.max(0, mix.output - mix.reasoning),
    mix.reasoning,
  ];
}

/** Every token of a mix (reasoning is already in output). */
export function mixTotal(mix: TokenMix): number {
  return mix.input_uncached + mix.cache_read + mix.cache_write_5m + mix.cache_write_1h + mix.output;
}

/** One stacked token-mix bar per session, in the given (cost) order. */
export function tokenMixRows(rows: readonly SessionPoint[]): BarDatum[] {
  return rows.map((row) => ({
    label: `${shortSession(row.session)} (${row.agent})`,
    values: tokenMixValues(row.tokens),
  }));
}

/** Sum of token mixes, e.g. every event of a session. */
export function sumTokenMix(mixes: readonly TokenMix[]): TokenMix {
  return mixes.reduce(
    (total, mix) => ({
      input_uncached: total.input_uncached + mix.input_uncached,
      cache_read: total.cache_read + mix.cache_read,
      cache_write_5m: total.cache_write_5m + mix.cache_write_5m,
      cache_write_1h: total.cache_write_1h + mix.cache_write_1h,
      output: total.output + mix.output,
      reasoning: total.reasoning + mix.reasoning,
    }),
    {
      input_uncached: 0,
      cache_read: 0,
      cache_write_5m: 0,
      cache_write_1h: 0,
      output: 0,
      reasoning: 0,
    },
  );
}

/** Cache-hit ratio 0-1 of a mix: reads over every input token; null without input. */
export function cacheHitRatio(mix: TokenMix): number | null {
  const input = mix.input_uncached + mix.cache_read + mix.cache_write_5m + mix.cache_write_1h;
  return input === 0 ? null : mix.cache_read / input;
}
