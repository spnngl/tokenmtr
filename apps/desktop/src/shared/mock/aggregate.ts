import type { Filters } from "../filters";
import type { Currency } from "../format";
import type { Usage } from "../schemas";
import { addDays, EUR_PER_USD, type Fact, type RawUsage, TODAY } from "./seed";

/** Local days, `end` exclusive. */
export interface DayRange {
  readonly start: string;
  readonly end: string;
}

export function rangeOf(range: Filters["range"]): DayRange {
  if (range.kind === "custom") {
    return { start: range.from, end: addDays(range.to, 1) };
  }
  const days = { today: 1, "7d": 7, "30d": 30, "90d": 90 }[range.preset];
  return { start: addDays(TODAY, 1 - days), end: addDays(TODAY, 1) };
}

export function inRange(date: string, range: DayRange): boolean {
  return date >= range.start && date < range.end;
}

/** Whether the profile filter (`agent/profile`, or a bare profile under any agent) picks this row. */
export function matchesProfile(filter: string | null, agent: string, profile: string): boolean {
  if (filter === null) {
    return true;
  }
  const slash = filter.indexOf("/");
  return slash === -1
    ? profile === filter
    : agent === filter.slice(0, slash) && profile === filter.slice(slash + 1);
}

/** Facts matching every filter dimension (the billing mode included). */
export function selectFacts(facts: readonly Fact[], filters: Filters): Fact[] {
  const range = rangeOf(filters.range);
  return facts.filter(
    (fact) =>
      inRange(fact.date, range) &&
      (filters.agent === null || fact.agent === filters.agent) &&
      matchesProfile(filters.profile, fact.agent, fact.profile) &&
      (filters.model === null || fact.model === filters.model) &&
      (filters.project === null || fact.project === filters.project) &&
      (filters.client === null || fact.client === filters.client) &&
      (filters.billing === "all" || fact.billing === filters.billing),
  );
}

export function convert(usdMicros: number, currency: Currency): number {
  return currency === "EUR" ? Math.round(usdMicros * EUR_PER_USD) : usdMicros;
}

const ZERO: RawUsage = {
  events: 0,
  input_uncached: 0,
  cache_read: 0,
  cache_write_5m: 0,
  cache_write_1h: 0,
  output: 0,
  reasoning: 0,
  valueUsd: 0,
  paidUsd: 0,
  extraUsd: 0,
  subscriptionValueUsd: 0,
  feeUsd: 0,
  reported: 0,
  estimated: 0,
  unpriced: 0,
};

export function sumRaw(items: readonly RawUsage[]): RawUsage {
  return items.reduce<RawUsage>(
    (total, item) => ({
      events: total.events + item.events,
      input_uncached: total.input_uncached + item.input_uncached,
      cache_read: total.cache_read + item.cache_read,
      cache_write_5m: total.cache_write_5m + item.cache_write_5m,
      cache_write_1h: total.cache_write_1h + item.cache_write_1h,
      output: total.output + item.output,
      reasoning: total.reasoning + item.reasoning,
      valueUsd: total.valueUsd + item.valueUsd,
      paidUsd: total.paidUsd + item.paidUsd,
      extraUsd: total.extraUsd + item.extraUsd,
      subscriptionValueUsd: total.subscriptionValueUsd + item.subscriptionValueUsd,
      feeUsd: total.feeUsd + item.feeUsd,
      reported: total.reported + item.reported,
      estimated: total.estimated + item.estimated,
      unpriced: total.unpriced + item.unpriced,
    }),
    ZERO,
  );
}

/** The wire `Usage` of a sum, money in `currency`. */
export function wireUsage(raw: RawUsage, currency: Currency): Usage {
  return {
    events: raw.events,
    input_uncached: raw.input_uncached,
    cache_read: raw.cache_read,
    cache_write_5m: raw.cache_write_5m,
    cache_write_1h: raw.cache_write_1h,
    output: raw.output,
    reasoning: raw.reasoning,
    total_tokens:
      raw.input_uncached +
      raw.cache_read +
      raw.cache_write_5m +
      raw.cache_write_1h +
      raw.output +
      raw.reasoning,
    api_value_micros: convert(raw.valueUsd, currency),
    api_value_usd_micros: raw.valueUsd,
    paid_micros: convert(raw.paidUsd, currency),
    extra_usage_micros: convert(raw.extraUsd, currency),
    subscription_value_micros: convert(raw.subscriptionValueUsd, currency),
    subscription_fee_micros: convert(raw.feeUsd, currency),
    reported_events: raw.reported,
    estimated_events: raw.estimated,
    unpriced_events: raw.unpriced,
    approximate: raw.unpriced > 0,
  };
}

export function groupBy<T>(items: readonly T[], key: (item: T) => string): Map<string, T[]> {
  const groups = new Map<string, T[]>();
  for (const item of items) {
    const bucket = groups.get(key(item));
    if (bucket === undefined) {
      groups.set(key(item), [item]);
    } else {
      bucket.push(item);
    }
  }
  return groups;
}

export function roi(raw: RawUsage): number | null {
  return raw.paidUsd > 0 ? raw.valueUsd / raw.paidUsd : null;
}
