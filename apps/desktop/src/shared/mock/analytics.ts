import type { Filters } from "../filters";
import type { Currency } from "../format";
import type {
  ActivityCalendar,
  ActivityDay,
  BillingAmounts,
  BillingPoint,
  BillingSeries,
  CostBucket,
  Granularity,
  SessionAnalytics,
  SessionModel,
  SessionPoint,
  SessionTimeline,
  TimelineEvent,
} from "../schemas";
import {
  convert,
  type DayRange,
  groupBy,
  rangeOf,
  selectFacts,
  sumRaw,
  wireUsage,
} from "./aggregate";
import {
  addDays,
  createRng,
  type Fact,
  hashString,
  type SessionSeed,
  TODAY,
  weekdayOf,
} from "./seed";
import { find, iso, matches } from "./sessions";

/**
 * The mock's clock: Paris summer time all year (UTC+2, every day 24 hours). The real backend
 * follows the zone's DST rules.
 */
const OFFSET_MS = 2 * 3_600_000;
const HOURS = 24;

function zero(): BillingAmounts {
  return {
    api_value_micros: 0,
    paid_micros: 0,
    subscription_fee_micros: 0,
    extra_usage_micros: 0,
    pay_as_you_go_micros: 0,
  };
}

function amountsOf(facts: readonly Fact[], currency: Currency): BillingAmounts {
  const sum = zero();
  for (const fact of facts) {
    const fee = convert(fact.feeUsd, currency);
    const extra = convert(fact.extraUsd, currency);
    const payg = convert(fact.paidUsd - fact.feeUsd - fact.extraUsd, currency);
    sum.api_value_micros += convert(fact.valueUsd, currency);
    sum.subscription_fee_micros += fee;
    sum.extra_usage_micros += extra;
    sum.pay_as_you_go_micros += payg;
    sum.paid_micros += fee + extra + payg;
  }
  return sum;
}

/** Share `index` of `parts` of `micros`; the shares sum to `micros` exactly. */
function share(micros: number, index: number, parts: number): number {
  return Math.floor((micros * (index + 1)) / parts) - Math.floor((micros * index) / parts);
}

function shareOf(amounts: BillingAmounts, index: number, parts: number): BillingAmounts {
  const fee = share(amounts.subscription_fee_micros, index, parts);
  const extra = share(amounts.extra_usage_micros, index, parts);
  const payg = share(amounts.pay_as_you_go_micros, index, parts);
  return {
    api_value_micros: share(amounts.api_value_micros, index, parts),
    paid_micros: fee + extra + payg,
    subscription_fee_micros: fee,
    extra_usage_micros: extra,
    pay_as_you_go_micros: payg,
  };
}

interface Bucket {
  /** Local wall-clock start, `YYYY-MM-DDTHH:MM:SS`. */
  readonly local: string;
  readonly minutes: number;
  readonly facts: readonly Fact[];
  /** Which of `parts` even shares of `facts` this bucket holds. */
  readonly index: number;
  readonly parts: number;
}

function bucketsOf(facts: readonly Fact[], range: DayRange, granularity: Granularity): Bucket[] {
  const monthly = granularity === "monthly";
  const keyOf = (date: string): string => (monthly ? `${date.slice(0, 7)}-01` : date);
  const byKey = groupBy(facts, (fact) => keyOf(fact.date));
  const keys: string[] = [];
  for (let date = range.start; date < range.end; date = addDays(date, 1)) {
    const key = keyOf(date);
    if (keys[keys.length - 1] !== key) {
      keys.push(key);
    }
  }
  return keys.flatMap((key) => {
    const rows = byKey.get(key) ?? [];
    if (granularity === "hourly") {
      return Array.from({ length: HOURS }, (_, hour) => ({
        local: `${key}T${String(hour).padStart(2, "0")}:00:00`,
        minutes: 60,
        facts: rows,
        index: hour,
        parts: HOURS,
      }));
    }
    let days = 1;
    while (monthly && addDays(key, days).slice(0, 7) === key.slice(0, 7)) {
      days += 1;
    }
    return [{ local: `${key}T00:00:00`, minutes: days * 1440, facts: rows, index: 0, parts: 1 }];
  });
}

/**
 * Paid versus API value per bucket. A day's fee rides on the Claude rows (`feeUsd`), extra
 * usage counts in paid only, and an hour gets an even 1/24 share of its day.
 */
export function billingSeriesOf(
  facts: readonly Fact[],
  extras: readonly Fact[],
  range: DayRange,
  currency: Currency,
  granularity: Granularity,
  splitByAgent: boolean,
): BillingSeries {
  let cumulativeApi = 0;
  let cumulativePaid = 0;
  const points = bucketsOf([...facts, ...extras], range, granularity).map(
    (bucket): BillingPoint => {
      const amounts = shareOf(amountsOf(bucket.facts, currency), bucket.index, bucket.parts);
      cumulativeApi += amounts.api_value_micros;
      cumulativePaid += amounts.paid_micros;
      const agents = splitByAgent
        ? Object.fromEntries(
            [...groupBy(bucket.facts, (fact) => fact.agent)].map(([agent, rows]) => [
              agent,
              shareOf(amountsOf(rows, currency), bucket.index, bucket.parts),
            ]),
          )
        : {};
      return {
        start: iso(Date.parse(`${bucket.local}Z`) - OFFSET_MS),
        local: bucket.local,
        minutes: bucket.minutes,
        ...amounts,
        savings_micros: amounts.api_value_micros - amounts.paid_micros,
        roi: amounts.paid_micros === 0 ? null : amounts.api_value_micros / amounts.paid_micros,
        cumulative_api_value_micros: cumulativeApi,
        cumulative_paid_micros: cumulativePaid,
        agents,
        approximate: bucket.facts.some((fact) => fact.estimated > 0 || fact.unpriced > 0),
      };
    },
  );
  return { currency, granularity, points };
}

/** Mirrors `COST_EDGES_MICROS` of the query crate. */
const COST_EDGES = [
  0, 10_000, 20_000, 50_000, 100_000, 200_000, 500_000, 1_000_000, 2_000_000, 5_000_000, 10_000_000,
  20_000_000, 50_000_000, 100_000_000,
] as const;
/** Mirrors `SESSION_ROWS_LIMIT`. */
const ROWS_LIMIT = 500;

function cacheHitPercent(seed: SessionSeed): number | null {
  const u = seed.usage;
  const input = u.input_uncached + u.cache_read + u.cache_write_5m + u.cache_write_1h;
  return input === 0 ? null : (u.cache_read * 100) / input;
}

function pointOf(seed: SessionSeed, currency: Currency): SessionPoint {
  const u = seed.usage;
  return {
    agent: seed.agent,
    session: seed.session,
    started_at: iso(seed.startedAtMs),
    duration_secs: seed.durationSecs,
    api_value_micros: convert(u.valueUsd, currency),
    paid_micros: convert(u.paidUsd, currency),
    cache_hit_percent: cacheHitPercent(seed),
    tokens: {
      input_uncached: u.input_uncached,
      cache_read: u.cache_read,
      cache_write_5m: u.cache_write_5m,
      cache_write_1h: u.cache_write_1h,
      output: u.output,
      reasoning: u.reasoning,
    },
    tool_calls: seed.toolCalls,
    tool_errors: seed.toolErrors,
  };
}

/** Session charts of the `sessions` matching `filters`. */
export function sessionAnalyticsOf(
  sessions: readonly SessionSeed[],
  filters: Filters,
  currency: Currency,
): SessionAnalytics {
  const matching = sessions.filter((seed) => matches(seed, filters));
  const points = matching.map((seed) => pointOf(seed, currency));
  const histogram = COST_EDGES.map((min, index): CostBucket => {
    const max = COST_EDGES[index + 1] ?? null;
    const inBucket = matching.filter((_, i) => {
      const api = points[i]?.api_value_micros ?? 0;
      return (index === 0 || api >= min) && (max === null || api < max);
    });
    // The mock's dominant model is the session's first model.
    const models = [...groupBy(inBucket, (seed) => seed.models[0] ?? "unknown")]
      .map(([model, seeds]) => ({ model, sessions: seeds.length }))
      .toSorted((a, b) => b.sessions - a.sessions || a.model.localeCompare(b.model));
    return { min_micros: min, max_micros: max, sessions: inBucket.length, models };
  });
  const localStarts = matching.map((seed) => new Date(seed.startedAtMs + OFFSET_MS));
  const range = rangeOf(filters.range);
  const days = [];
  for (let day = range.start; day < range.end; day = addDays(day, 1)) {
    const sessions = localStarts.filter((at) => at.toISOString().slice(0, 10) === day).length;
    days.push({ day, sessions });
  }
  const starts = Array.from({ length: 7 * HOURS }, (_, cell) => {
    const weekday = Math.floor(cell / HOURS) + 1;
    const hour = cell % HOURS;
    const sessions = localStarts.filter(
      (at) => (at.getUTCDay() || 7) === weekday && at.getUTCHours() === hour,
    ).length;
    return { weekday, hour, sessions };
  });
  const rows = points
    .toSorted(
      (a, b) => b.api_value_micros - a.api_value_micros || a.session.localeCompare(b.session),
    )
    .slice(0, ROWS_LIMIT);
  return { currency, total: points.length, rows, histogram, days, starts };
}

/** Mirrors `CALENDAR_WEEKS` of the query crate. */
const CALENDAR_WEEKS = 105;

/** Daily tokens and started sessions from the Monday 104 weeks before today's week to today. */
export function activityCalendarOf(
  facts: readonly Fact[],
  sessions: readonly SessionSeed[],
  filters: Filters,
): ActivityCalendar {
  const start = addDays(TODAY, 1 - weekdayOf(TODAY) - 7 * (CALENDAR_WEEKS - 1));
  const scoped: Filters = { ...filters, range: { kind: "custom", from: start, to: TODAY } };
  const tokens = groupBy(selectFacts(facts, scoped), (fact) => fact.date);
  const starts = groupBy(
    sessions.filter((seed) => matches(seed, scoped)),
    (seed) => new Date(seed.startedAtMs + OFFSET_MS).toISOString().slice(0, 10),
  );
  const days: ActivityDay[] = [];
  for (let day = start; day <= TODAY; day = addDays(day, 1)) {
    days.push({
      day,
      total_tokens: wireUsage(sumRaw(tokens.get(day) ?? []), "USD").total_tokens,
      sessions: starts.get(day)?.length ?? 0,
    });
  }
  return { days };
}

const TIMELINE_TOOLS: readonly { readonly name: string; readonly server: string | null }[] = [
  { name: "Bash", server: null },
  { name: "Read", server: null },
  { name: "Edit", server: null },
  { name: "mcp__github__create_issue", server: "github" },
];
const MAX_EVENTS = 60;

/** One session over time: its usage spread over up to 60 events, a compaction past halfway. */
export function sessionTimelineOf(
  agent: string,
  session: string,
  currency: Currency,
): SessionTimeline {
  const seed = find(agent, session);
  if (seed === undefined) {
    return { currency, events: [], events_truncated: false, signals: [], tools: [], models: [] };
  }
  const rng = createRng(hashString(session));
  const u = seed.usage;
  const count = Math.min(u.events, MAX_EVENTS);
  const at = (fraction: number): string =>
    iso(seed.startedAtMs + Math.round(seed.durationSecs * 1000 * fraction));
  let cumulativeApi = 0;
  let cumulativePaid = 0;
  const events = Array.from({ length: count }, (_, index): TimelineEvent => {
    const api = convert(share(u.valueUsd, index, count), currency);
    const paid = convert(share(u.paidUsd, index, count), currency);
    cumulativeApi += api;
    cumulativePaid += paid;
    return {
      at: at(index / count),
      model: seed.models[index % seed.models.length] ?? "unknown",
      tokens: {
        input_uncached: share(u.input_uncached, index, count),
        cache_read: share(u.cache_read, index, count),
        cache_write_5m: share(u.cache_write_5m, index, count),
        cache_write_1h: share(u.cache_write_1h, index, count),
        output: share(u.output, index, count),
        reasoning: share(u.reasoning, index, count),
      },
      api_value_micros: api,
      paid_micros: paid,
      cumulative_api_value_micros: cumulativeApi,
      cumulative_paid_micros: cumulativePaid,
    };
  });
  const signals: SessionTimeline["signals"] = Array.from(
    { length: seed.apiErrors },
    (_, index): SessionTimeline["signals"][number] => ({
      at: at((index + 1) / (seed.apiErrors + 2)),
      kind: "api_error",
    }),
  );
  if (count > 30) {
    signals.push({ at: at(0.6), kind: "compaction" });
    signals.sort((a, b) => a.at.localeCompare(b.at));
  }
  const tools = TIMELINE_TOOLS.map((tool, index) => ({
    tool_name: tool.name,
    mcp_server: tool.server,
    calls: share(seed.toolCalls, index, TIMELINE_TOOLS.length),
    failed: share(seed.toolErrors, index, TIMELINE_TOOLS.length),
  }))
    .filter((tool) => tool.calls > 0 && rng() < 0.95)
    .toSorted((a, b) => b.calls - a.calls || a.tool_name.localeCompare(b.tool_name));
  return {
    currency,
    events,
    events_truncated: u.events > MAX_EVENTS,
    signals,
    tools,
    models: modelsOf(events),
  };
}

/** The events' usage per model, highest API value first. */
function modelsOf(events: readonly TimelineEvent[]): SessionModel[] {
  return [...groupBy(events, (event) => event.model)]
    .map(([model, group]): SessionModel => {
      const tokens = { ...ZERO_TOKENS };
      for (const event of group) {
        for (const key of TOKEN_KEYS) {
          tokens[key] += event.tokens[key];
        }
      }
      const api = group.reduce((sum, event) => sum + event.api_value_micros, 0);
      return { model, events: group.length, tokens, api_value_micros: api, approximate: false };
    })
    .toSorted((a, b) => b.api_value_micros - a.api_value_micros || a.model.localeCompare(b.model));
}

const TOKEN_KEYS = [
  "input_uncached",
  "cache_read",
  "cache_write_5m",
  "cache_write_1h",
  "output",
  "reasoning",
] as const;
const ZERO_TOKENS: SessionModel["tokens"] = {
  input_uncached: 0,
  cache_read: 0,
  cache_write_5m: 0,
  cache_write_1h: 0,
  output: 0,
  reasoning: 0,
};
