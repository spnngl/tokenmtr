import { defaultFilters } from "../filters";
import type { Currency } from "../format";
import type { AgentRecap, Period, RecapSnapshot } from "../schemas";
import { STATUS_SCHEMA_VERSION } from "../schemas";
import { rangeOf, sumRaw } from "./aggregate";
import { quotaLatestOf } from "./quota";
import { budgetsOf, extraFactsFor, factsFor, totalsOf } from "./reports";
import { addDays, NOW_ISO, NOW_MS, SESSIONS, TODAY } from "./seed";
import { agentInfos, systemHealthOf } from "./system";

/** Trailing windows: the mock clock sits on a Monday, a calendar week would equal `today`. */
const PERIOD_DAYS: Readonly<Record<Period, number>> = { today: 1, week: 7, month: 15 };
const PROJECTION_DAYS: Readonly<Record<Period, number>> = { today: 1, week: 7, month: 30 };
/** Share of the day already elapsed at `NOW_MS` (12:00 UTC). */
const DAY_FRACTION = 0.5;

function recapFor(
  agent: string | null,
  period: Period,
  currency: Currency,
  quotaCapable: boolean,
  hasLive: boolean,
  extraUsage: boolean,
): AgentRecap {
  const days = PERIOD_DAYS[period];
  const filters = {
    ...defaultFilters(currency),
    agent,
    range: { kind: "custom", from: addDays(TODAY, 1 - days), to: TODAY } as const,
  };
  const facts = factsFor(filters);
  const range = rangeOf(filters.range);
  const sessions = SESSIONS.filter(
    (session) =>
      (agent === null || session.agent === agent) &&
      new Date(session.startedAtMs).toISOString().slice(0, 10) >= range.start,
  ).length;
  const sparkFilters = {
    ...filters,
    range: { kind: "custom", from: addDays(TODAY, -6), to: TODAY },
  } as const;
  const sparkFacts = factsFor(sparkFilters);
  const sparkline = Array.from({ length: 7 }, (_, index) => {
    const date = addDays(TODAY, index - 6);
    const value = sumRaw(sparkFacts.filter((fact) => fact.date === date)).valueUsd;
    return { date, api_value_micros: currency === "EUR" ? Math.round(value * 0.92) : value };
  });
  const totals = totalsOf(facts, currency, sessions, extraUsage ? extraFactsFor(filters) : []);
  const elapsedDays = days - 1 + DAY_FRACTION;
  const topModel = [...facts]
    .sort((a, b) => b.valueUsd - a.valueUsd)
    .map((fact) => fact.model)
    .at(0);
  const quota = quotaCapable
    ? quotaLatestOf({ ...defaultFilters(currency), agent }, extraUsage)
    : [];
  return {
    agent,
    period,
    range,
    currency,
    currency_fallback: false,
    totals,
    projected_paid_micros: Math.round((totals.paid_micros / elapsedDays) * PROJECTION_DAYS[period]),
    quota,
    budgets: budgetsOf().filter((budget) => agent === null || budget.id.includes(agent)),
    sparkline,
    live: hasLive
      ? { tokens_per_minute: 12_500, api_value_per_hour_usd_micros: 3_200_000, active_sessions: 2 }
      : null,
    top_model: topModel ?? null,
    last_activity:
      facts.length === 0 ? null : new Date(NOW_MS - 2 * 60_000).toISOString().replace(".000Z", "Z"),
  };
}

/** `extraUsage`: claude/default has Claude extra usage switched on. */
export function recapOf(
  period: Period,
  currency: Currency,
  empty: boolean,
  extraUsage = true,
): RecapSnapshot {
  const agents = empty ? [] : agentInfos(new Set()).filter((info) => info.followed);
  const perAgent = agents.map((info) =>
    recapFor(
      info.agent,
      period,
      currency,
      info.capabilities.quota,
      info.agent === "claude",
      extraUsage,
    ),
  );
  const all =
    agents.length > 1
      ? [
          recapFor(
            null,
            period,
            currency,
            agents.some((info) => info.capabilities.quota),
            true,
            extraUsage,
          ),
        ]
      : [];
  return {
    status_schema: STATUS_SCHEMA_VERSION,
    generated_at: NOW_ISO,
    system: systemHealthOf(),
    tabs: [...all, ...perAgent],
  };
}
