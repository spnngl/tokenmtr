import type { Filters } from "../filters";
import type { Currency } from "../format";
import type { AdvisorReport, QuotaHistory, QuotaMeter, QuotaWindow } from "../schemas";
import { convert, matchesProfile, rangeOf, sumRaw } from "./aggregate";
import {
  addDays,
  createRng,
  EXTRA_FACTS,
  EXTRA_USAGE_LIMIT_USD_MICROS,
  FACTS,
  hashString,
  NOW_MS,
  TODAY,
} from "./seed";

const HOUR_MS = 3_600_000;

interface WindowSeed {
  readonly window: string;
  /** What the backend's `WindowId::label` returns. */
  readonly label: string;
  readonly cycleMs: number;
  /** The window resets this long after `NOW_MS`. */
  readonly resetsInMs: number;
  /** Used percent at the end of a cycle. */
  readonly peak: number;
}

const WINDOWS: readonly WindowSeed[] = [
  {
    window: "five_hour",
    label: "5-hour",
    cycleMs: 5 * HOUR_MS,
    resetsInMs: 2.25 * HOUR_MS,
    peak: 135,
  },
  {
    window: "seven_day",
    label: "7-day",
    cycleMs: 7 * 24 * HOUR_MS,
    resetsInMs: 12 * HOUR_MS,
    peak: 100,
  },
  {
    window: "weekly_scoped:fable",
    label: "Weekly · Fable",
    cycleMs: 7 * 24 * HOUR_MS,
    resetsInMs: 12 * HOUR_MS,
    peak: 80,
  },
];

function iso(ms: number): string {
  return new Date(ms).toISOString().replace(".000Z", "Z");
}

/** Used percent of a window at `atMs`: a sawtooth that climbs to `peak` and resets. */
function usedAt(seed: WindowSeed, atMs: number): number {
  const resetAnchor = NOW_MS + seed.resetsInMs;
  const into =
    (((atMs - (resetAnchor - seed.cycleMs)) % seed.cycleMs) + seed.cycleMs) % seed.cycleMs;
  return Math.min(100, (into / seed.cycleMs) * seed.peak);
}

function windowView(seed: WindowSeed): QuotaWindow {
  const used = usedAt(seed, NOW_MS);
  return {
    window: seed.window,
    label: seed.label,
    used_percent: used,
    resets_at: iso(NOW_MS + seed.resetsInMs),
    resets_in_secs: Math.round(seed.resetsInMs / 1000),
    eta:
      seed.window === "five_hour"
        ? { kind: "will_hit", at: iso(NOW_MS + 1.67 * HOUR_MS) }
        : { kind: "wont_hit" },
    spend: null,
  };
}

/** claude/default's monthly extra-usage window: this month's `EXTRA_FACTS` over the cap. */
function extraUsageWindow(currency: Currency): QuotaWindow {
  const month = TODAY.slice(0, 7);
  const spentUsd = sumRaw(EXTRA_FACTS.filter((fact) => fact.date.startsWith(month))).extraUsd;
  return {
    window: "extra_usage",
    label: "Extra usage",
    used_percent: (spentUsd / EXTRA_USAGE_LIMIT_USD_MICROS) * 100,
    resets_at: null,
    resets_in_secs: null,
    eta: { kind: "not_enough_data" },
    spend: {
      currency,
      spent_micros: convert(spentUsd, currency),
      limit_micros: convert(EXTRA_USAGE_LIMIT_USD_MICROS, currency),
      approximate: false,
    },
  };
}

function metersOf(currency: Currency, extraUsage: boolean): QuotaMeter[] {
  return [
    {
      agent: "claude",
      profile: "default",
      fetched_at: iso(NOW_MS - 60_000),
      age_secs: 60,
      status: "ok",
      account: "dev@example.com",
      windows: [...WINDOWS.map(windowView), ...(extraUsage ? [extraUsageWindow(currency)] : [])],
    },
    {
      agent: "claude",
      profile: "work",
      fetched_at: iso(NOW_MS - 3 * 24 * HOUR_MS),
      age_secs: 3 * 24 * 3600,
      status: "token_expired",
      account: "dev@acme.example",
      windows: [],
    },
  ];
}

/** Latest meters; `extraUsage` adds claude/default's extra-usage window (off: Claude omits it). */
export function quotaLatestOf(filters: Filters, extraUsage = true): QuotaMeter[] {
  return metersOf(filters.currency, extraUsage).filter(
    (meter) =>
      (filters.agent === null || meter.agent === filters.agent) &&
      matchesProfile(filters.profile, meter.agent, meter.profile),
  );
}

const MAX_POINTS = 380;

export function quotaHistoryOf(filters: Filters, agent: string): QuotaHistory[] {
  const range = rangeOf(filters.range);
  const startMs = Date.parse(`${range.start}T00:00:00Z`);
  const endMs = Math.min(Date.parse(`${range.end}T00:00:00Z`), NOW_MS);
  const stepMs = Math.max(15 * 60_000, Math.ceil((endMs - startMs) / MAX_POINTS));
  return quotaLatestOf({ ...filters, agent })
    .filter((meter) => meter.status === "ok")
    .flatMap((meter) =>
      WINDOWS.map((seed): QuotaHistory => {
        const rng = createRng(hashString(`${meter.profile}:${seed.window}`));
        const points = [];
        for (let at = startMs; at <= endMs; at += stepMs) {
          points.push({ at: iso(at), used_percent: Math.max(0, usedAt(seed, at) + rng() * 2 - 1) });
        }
        const resets: string[] = [];
        for (let at = NOW_MS + seed.resetsInMs - seed.cycleMs; at >= startMs; at -= seed.cycleMs) {
          if (at <= endMs) {
            resets.unshift(iso(at));
          }
        }
        return {
          agent: meter.agent,
          profile: meter.profile,
          window: seed.window,
          label: seed.label,
          points,
          resets,
        };
      }),
    );
}

export function advisorOf(filters: Filters): AdvisorReport[] {
  if (filters.agent !== null && filters.agent !== "claude") {
    return [];
  }
  const since = addDays(TODAY, -29);
  const monthly = sumRaw(
    FACTS.filter((fact) => fact.agent === "claude" && fact.date >= since),
  ).valueUsd;
  return [
    {
      agent: "claude",
      profile: "default",
      current_plan: "max_5x",
      snapshots: 1_240,
      monthly_api_value_usd_micros: monthly,
      advice: {
        advice: "recommend",
        verdict: { verdict: "keep" },
        current: "max_5x",
        headroom: 80,
        fits: [
          {
            plan: "pro",
            monthly_fee: 20_000_000,
            five_hour_peak: 310,
            seven_day_peak: 355,
            fits: false,
          },
          {
            plan: "max_5x",
            monthly_fee: 100_000_000,
            five_hour_peak: 62,
            seven_day_peak: 71,
            fits: true,
          },
          {
            plan: "max_20x",
            monthly_fee: 200_000_000,
            five_hour_peak: 15.5,
            seven_day_peak: 17.8,
            fits: true,
          },
        ],
      },
    },
  ];
}
