import type { Filters } from "../filters";
import type { Currency } from "../format";
import type {
  BillingReport,
  BranchRow,
  Budget,
  GroupBy,
  Heatmap,
  ModelRow,
  ProjectModelsRow,
  ProjectRow,
  Series,
  SignalsReport,
  ToolRow,
  ToolsReport,
  Totals,
} from "../schemas";
import { convert, type DayRange, groupBy, roi, selectFacts, sumRaw, wireUsage } from "./aggregate";
import {
  addDays,
  createRng,
  EUR_PER_USD,
  EXTRA_FACTS,
  FACTS,
  type Fact,
  hashString,
  type RawUsage,
  SESSIONS,
  TODAY,
  weekdayOf,
} from "./seed";

const UNASSIGNED = "(unassigned)";

/** Overview KPIs; `extras` (extra-usage rows) add to paid only. */
export function totalsOf(
  facts: readonly Fact[],
  currency: Currency,
  sessions: number,
  extras: readonly Fact[] = [],
): Totals {
  const raw = sumRaw([...facts, ...extras]);
  const input = raw.input_uncached + raw.cache_read + raw.cache_write_5m + raw.cache_write_1h;
  return {
    currency,
    currency_fallback: false,
    ...wireUsage(raw, currency),
    cached_input_percent: input === 0 ? 0 : (raw.cache_read / input) * 100,
    cache_savings_micros:
      raw.cache_read === 0 ? null : convert(Math.round(raw.cache_read * 2.7), currency),
    roi: roi(raw),
    sessions,
  };
}

function bucketFor(range: DayRange): "day" | "week" | "month" {
  const days = (Date.parse(range.end) - Date.parse(range.start)) / 86_400_000;
  if (days <= 120) {
    return "day";
  }
  return days <= 730 ? "week" : "month";
}

function bucketStart(date: string, bucket: "day" | "week" | "month"): string {
  if (bucket === "day") {
    return date;
  }
  return bucket === "week" ? addDays(date, 1 - weekdayOf(date)) : `${date.slice(0, 7)}-01`;
}

function groupKey(fact: Fact, by: GroupBy): string {
  switch (by) {
    case "none":
      return "(none)";
    case "agent":
      return fact.agent;
    case "model":
      return fact.model;
    case "project":
      // The backend's `NO_GROUP`.
      return fact.project ?? "(none)";
    default:
      by satisfies never;
      throw new Error("unreachable group");
  }
}

/** `extras` (extra-usage rows) are in the bucket totals and the agent split, in no model or project. */
export function seriesOf(
  facts: readonly Fact[],
  range: DayRange,
  currency: Currency,
  by: GroupBy,
  extras: readonly Fact[] = [],
): Series {
  const bucket = bucketFor(range);
  const byStart = groupBy(facts, (fact) => bucketStart(fact.date, bucket));
  const extrasByStart = groupBy(extras, (fact) => bucketStart(fact.date, bucket));
  const grouped = by === "none" || by === "agent";
  const starts: string[] = [];
  for (let date = range.start; date < range.end; date = addDays(date, 1)) {
    const start = bucketStart(date, bucket);
    if (starts[starts.length - 1] !== start) {
      starts.push(start);
    }
  }
  let cumulativeValue = 0;
  let cumulativePaid = 0;
  const points = starts.map((start) => {
    const extraRows = extrasByStart.get(start) ?? [];
    const rows = [...(byStart.get(start) ?? []), ...(grouped ? extraRows : [])];
    const raw = sumRaw(grouped ? rows : [...rows, ...extraRows]);
    cumulativeValue += convert(raw.valueUsd, currency);
    cumulativePaid += convert(raw.paidUsd, currency);
    const groups = Object.fromEntries(
      [...groupBy(rows, (fact) => groupKey(fact, by))].map(([key, members]) => {
        const sum = wireUsage(sumRaw(members), currency);
        return [
          key,
          {
            total_tokens: sum.total_tokens,
            api_value_micros: sum.api_value_micros,
            paid_micros: sum.paid_micros,
          },
        ];
      }),
    );
    return {
      start,
      minutes: Math.round(raw.events * 2.5),
      ...wireUsage(raw, currency),
      cumulative_api_value_micros: cumulativeValue,
      cumulative_paid_micros: cumulativePaid,
      groups,
    };
  });
  return { currency, bucket, group_by: by, points };
}

export function modelsOf(facts: readonly Fact[], currency: Currency): ModelRow[] {
  const total = sumRaw(facts).valueUsd;
  return [...groupBy(facts, (fact) => fact.model)]
    .map(([model, rows]): ModelRow => {
      const raw = sumRaw(rows);
      const known = raw.reported + raw.estimated;
      return {
        model,
        ...wireUsage(raw, currency),
        share_percent: total === 0 ? 0 : (raw.valueUsd / total) * 100,
        reported_vs_estimated_percent: known === 0 ? null : (raw.reported / known) * 100,
        divergent: model === "gemini-2.5-pro",
      };
    })
    .sort((a, b) => b.api_value_usd_micros - a.api_value_usd_micros);
}

export function projectsOf(facts: readonly Fact[], currency: Currency): ProjectRow[] {
  const total = sumRaw(facts).valueUsd;
  return [...groupBy(facts, (fact) => fact.project ?? UNASSIGNED)]
    .map(([key, rows]): ProjectRow => {
      const raw = sumRaw(rows);
      const project = key === UNASSIGNED ? null : key;
      return {
        project,
        name: project,
        client: rows[0]?.client ?? null,
        ...wireUsage(raw, currency),
        share_percent: total === 0 ? 0 : (raw.valueUsd / total) * 100,
      };
    })
    .sort((a, b) => b.api_value_usd_micros - a.api_value_usd_micros);
}

/** Projects split by model, biggest API value first (like `project_models`). */
export function projectModelsOf(facts: readonly Fact[], currency: Currency): ProjectModelsRow[] {
  return [...groupBy(facts, (fact) => fact.project ?? UNASSIGNED)]
    .map(([key, rows]): ProjectModelsRow => {
      const project = key === UNASSIGNED ? null : key;
      const models = [...groupBy(rows, (fact) => fact.model)]
        .map(([model, members]) => ({ model, ...wireUsage(sumRaw(members), currency) }))
        .sort((a, b) => b.api_value_micros - a.api_value_micros);
      return { project, name: project, models };
    })
    .sort((a, b) => apiOf(b) - apiOf(a));
}

function apiOf(row: ProjectModelsRow): number {
  return row.models.reduce((sum, slice) => sum + slice.api_value_micros, 0);
}

const BRANCH_SPLIT: readonly { readonly branch: string | null; readonly share: number }[] = [
  { branch: "main", share: 0.6 },
  { branch: "feature/budgets", share: 0.3 },
  { branch: null, share: 0.1 },
];

function scaleRaw(raw: RawUsage, share: number): RawUsage {
  const scale = (value: number): number => Math.round(value * share);
  return {
    events: scale(raw.events),
    input_uncached: scale(raw.input_uncached),
    cache_read: scale(raw.cache_read),
    cache_write_5m: scale(raw.cache_write_5m),
    cache_write_1h: scale(raw.cache_write_1h),
    output: scale(raw.output),
    reasoning: scale(raw.reasoning),
    valueUsd: scale(raw.valueUsd),
    paidUsd: scale(raw.paidUsd),
    subscriptionValueUsd: scale(raw.subscriptionValueUsd),
    feeUsd: scale(raw.feeUsd),
    reported: scale(raw.reported),
    estimated: scale(raw.estimated),
    unpriced: scale(raw.unpriced),
    extraUsd: scale(raw.extraUsd),
  };
}

export function branchesOf(facts: readonly Fact[], currency: Currency): BranchRow[] {
  // Like the backend, no usage means no branch rows rather than zeroed ones.
  if (facts.length === 0) {
    return [];
  }
  const raw = sumRaw(facts);
  return BRANCH_SPLIT.map(({ branch, share }) => ({
    branch,
    ...wireUsage(scaleRaw(raw, share), currency),
  }));
}

/** Working-hours bell, Paris time. */
const HOUR_WEIGHTS = [
  0, 0, 0, 0, 0, 0.1, 0.3, 1, 3, 6, 8, 8, 5, 4, 8, 9, 8, 6, 3, 2, 1.5, 1, 0.5, 0.2,
];

export function heatmapOf(facts: readonly Fact[], currency: Currency): Heatmap {
  const weightSum = HOUR_WEIGHTS.reduce((sum, weight) => sum + weight, 0);
  const cells = Array.from({ length: 7 }, (_, weekdayIndex) =>
    HOUR_WEIGHTS.map((weight, hour) => {
      const rows = facts.filter((fact) => weekdayOf(fact.date) === weekdayIndex + 1);
      const raw = scaleRaw(sumRaw(rows), weight / weightSum);
      const sum = wireUsage(raw, currency);
      return {
        weekday: weekdayIndex + 1,
        hour,
        events: sum.events,
        total_tokens: sum.total_tokens,
        api_value_micros: sum.api_value_micros,
      };
    }),
  ).flat();
  return { currency, cells };
}

const TOOL_SHARES: readonly {
  tool: string;
  server: string | null;
  share: number;
  errors: number;
}[] = [
  { tool: "Bash", server: null, share: 0.3, errors: 0.06 },
  { tool: "Read", server: null, share: 0.25, errors: 0.01 },
  { tool: "Edit", server: null, share: 0.2, errors: 0.03 },
  { tool: "Grep", server: null, share: 0.12, errors: 0.005 },
  { tool: "mcp__github__create_issue", server: "github", share: 0.06, errors: 0.12 },
  { tool: "mcp__linear__list_issues", server: "linear", share: 0.04, errors: 0.02 },
  { tool: "Write", server: null, share: 0.03, errors: 0.01 },
];

export function toolsOf(facts: readonly Fact[]): ToolsReport {
  const events = sumRaw(facts).events;
  const rows: ToolRow[] = TOOL_SHARES.map(({ tool, server, share, errors }) => {
    const calls = Math.round(events * 1.4 * share);
    const failed = Math.round(calls * errors);
    return {
      tool_name: tool,
      mcp_server: server,
      calls,
      errors: failed,
      error_rate_percent: calls === 0 ? 0 : (failed / calls) * 100,
      sessions: Math.round(calls / 25),
    };
  });
  const servers = ["github", "linear"].map((server): ToolRow => {
    const own = rows.filter((row) => row.mcp_server === server);
    const calls = own.reduce((sum, row) => sum + row.calls, 0);
    const errors = own.reduce((sum, row) => sum + row.errors, 0);
    return {
      tool_name: null,
      mcp_server: server,
      calls,
      errors,
      error_rate_percent: calls === 0 ? 0 : (errors / calls) * 100,
      sessions: own.reduce((sum, row) => sum + row.sessions, 0),
    };
  });
  const shares = sharesOf(
    [...groupBy(facts, (fact) => fact.model)].map(([model, own]) => [model, sumRaw(own).events]),
  );
  const byModel = rows
    .flatMap((row) =>
      shares.map(({ model, share }) => ({
        tool_name: row.tool_name ?? "",
        mcp_server: row.mcp_server,
        model,
        calls: Math.round(row.calls * share),
        errors: Math.round(row.errors * share),
      })),
    )
    .filter((row) => row.calls > 0)
    .toSorted((a, b) => b.calls - a.calls || a.tool_name.localeCompare(b.tool_name));
  return { tools: rows, mcp_servers: servers, by_model: byModel };
}

/** Each model's share of the total weight, largest first. */
function sharesOf(
  weights: readonly (readonly [string, number])[],
): { model: string; share: number }[] {
  const total = weights.reduce((sum, [, weight]) => sum + weight, 0);
  return weights
    .map(([model, weight]) => ({ model, share: total === 0 ? 0 : weight / total }))
    .toSorted((a, b) => b.share - a.share || a.model.localeCompare(b.model));
}

export function signalsOf(range: DayRange): SignalsReport {
  const days: SignalsReport["days"] = [];
  for (let date = range.start; date < range.end && date <= TODAY; date = addDays(date, 1)) {
    const rng = createRng(hashString(date));
    days.push({
      date,
      api_errors: Math.floor(rng() * rng() * 6),
      compactions: Math.floor(rng() * 4),
    });
  }
  const inRangeSessions = SESSIONS.filter((session) => {
    const day = new Date(session.startedAtMs).toISOString().slice(0, 10);
    return day >= range.start && day < range.end;
  });
  const top: SignalsReport["top_sessions"] = inRangeSessions.slice(0, 5).map((session, index) => ({
    agent: session.agent,
    session: session.session,
    compactions: 5 - index,
  }));
  const apiErrors = days.reduce((sum, day) => sum + day.api_errors, 0);
  const compactions = days.reduce((sum, day) => sum + day.compactions, 0);
  const shares = sharesOf(
    [...groupBy(inRangeSessions, (session) => session.models[0] ?? "unknown")].map(
      ([model, own]) => [model, own.length],
    ),
  );
  const models = shares
    .map(({ model, share }) => ({
      model,
      api_errors: Math.round(apiErrors * share),
      compactions: Math.round(compactions * share),
    }))
    .filter((row) => row.api_errors + row.compactions > 0);
  return { days, api_errors: apiErrors, compactions, top_sessions: top, models };
}

/** `extras` (extra-usage rows) count in the total and the months, in no billing line. */
export function billingOf(
  facts: readonly Fact[],
  currency: Currency,
  extras: readonly Fact[] = [],
): BillingReport {
  const subscribed = facts.filter((fact) => fact.billing === "subscription");
  const api = facts.filter((fact) => fact.billing === "api");
  const subRaw = sumRaw(subscribed);
  const all = [...facts, ...extras];
  const months = [...groupBy(all, (fact) => `${fact.date.slice(0, 7)}-01`)]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([month, rows]) => {
      const raw = sumRaw(rows);
      return { month, ...wireUsage(raw, currency), roi: roi(raw) };
    });
  return {
    currency,
    total: wireUsage(sumRaw(all), currency),
    api: wireUsage(sumRaw(api), currency),
    subscriptions:
      subscribed.length === 0
        ? []
        : [
            {
              id: "claude-max",
              provider: "anthropic",
              plan: "max_5x",
              fee_micros: 100_000_000,
              fee_currency: "USD",
              period: "monthly",
              start: "2026-01-01",
              end: null,
              usage: wireUsage(subRaw, currency),
              roi: roi(subRaw),
            },
          ],
    months,
    roi: roi(sumRaw(all)),
  };
}

interface BudgetSeed {
  readonly id: string;
  readonly period: "day" | "week" | "month";
  readonly measure: "paid" | "api_value";
  readonly currency: Currency;
  /** Limit as a share of what is spent so far: above 1 means under budget. */
  readonly headroom: number;
  readonly agent: string | null;
}

const BUDGET_SEEDS: readonly BudgetSeed[] = [
  {
    id: "monthly-paid",
    period: "month",
    measure: "paid",
    currency: "USD",
    headroom: 1.6,
    agent: null,
  },
  {
    id: "weekly-claude-value",
    period: "week",
    measure: "api_value",
    currency: "USD",
    headroom: 0.75,
    agent: "claude",
  },
  { id: "daily-eur", period: "day", measure: "paid", currency: "EUR", headroom: 1.5, agent: null },
];

const PERIOD_DAYS = { day: 1, week: 7, month: 30 } as const;

/** Limit of a budget shown without an FX rate: its spend-based seed limit needs the rate. */
const NO_RATE_LIMIT_MICROS = 20_000_000;

function periodStart(period: BudgetSeed["period"]): string {
  switch (period) {
    case "day":
      return TODAY;
    case "week":
      return addDays(TODAY, 1 - weekdayOf(TODAY));
    case "month":
      return `${TODAY.slice(0, 7)}-01`;
    default:
      period satisfies never;
      throw new Error("unreachable period");
  }
}

/** With `noFxRates`, EUR budgets cannot be measured yet (as before the first ECB fetch). */
export function budgetsOf(noFxRates = false): Budget[] {
  return BUDGET_SEEDS.map((seed): Budget => {
    const start = periodStart(seed.period);
    const base = {
      id: seed.id,
      period: seed.period,
      measure: seed.measure,
      currency: seed.currency,
      period_start: start,
      period_end: addDays(start, PERIOD_DAYS[seed.period]),
    };
    if (noFxRates && seed.currency === "EUR") {
      return { state: "no_fx_rate", ...base, limit_micros: NO_RATE_LIMIT_MICROS };
    }
    const rows = FACTS.filter(
      (fact) => fact.date >= start && (seed.agent === null || fact.agent === seed.agent),
    );
    const raw = sumRaw(rows);
    const spentUsd = seed.measure === "paid" ? raw.paidUsd : raw.valueUsd;
    const spent = seed.currency === "EUR" ? Math.round(spentUsd * EUR_PER_USD) : spentUsd;
    const limit = Math.max(1_000_000, Math.round((spent * seed.headroom) / 1_000_000) * 1_000_000);
    const elapsed = Math.max(1, (Date.parse(TODAY) - Date.parse(start)) / 86_400_000 + 1);
    const projected = Math.round((spent / elapsed) * PERIOD_DAYS[seed.period]);
    return {
      state: "measured",
      ...base,
      limit_micros: limit,
      spent_micros: spent,
      percent: Math.floor((spent / limit) * 100),
      projected_micros: projected,
      projected_percent: Math.floor((projected / limit) * 100),
    };
  });
}

export function factsFor(filters: Filters): Fact[] {
  return selectFacts(FACTS, filters);
}

/** Extra-usage rows under `filters`: a model, project or client filter, or `subscription`, drops them. */
export function extraFactsFor(filters: Filters): Fact[] {
  return selectFacts(EXTRA_FACTS, filters);
}
