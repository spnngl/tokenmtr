import type { Filters } from "../filters";
import type { AdvisorReport, QuotaHistory, QuotaMeter, QuotaWindow } from "../schemas";
import { rangeOf, sumRaw } from "./aggregate";
import { addDays, createRng, FACTS, hashString, NOW_MS, TODAY } from "./seed";

const HOUR_MS = 3_600_000;

interface WindowSeed {
  readonly window: string;
  readonly cycleMs: number;
  /** The window resets this long after `NOW_MS`. */
  readonly resetsInMs: number;
  /** Used percent at the end of a cycle. */
  readonly peak: number;
}

const WINDOWS: readonly WindowSeed[] = [
  { window: "five_hour", cycleMs: 5 * HOUR_MS, resetsInMs: 2.25 * HOUR_MS, peak: 135 },
  { window: "seven_day", cycleMs: 7 * 24 * HOUR_MS, resetsInMs: 12 * HOUR_MS, peak: 100 },
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
    used_percent: used,
    resets_at: iso(NOW_MS + seed.resetsInMs),
    resets_in_secs: Math.round(seed.resetsInMs / 1000),
    eta:
      seed.window === "five_hour"
        ? { kind: "will_hit", at: iso(NOW_MS + 1.67 * HOUR_MS) }
        : { kind: "wont_hit" },
  };
}

const METERS: readonly QuotaMeter[] = [
  {
    agent: "claude",
    profile: "default",
    fetched_at: iso(NOW_MS - 60_000),
    age_secs: 60,
    status: "ok",
    account: "dev@example.com",
    windows: WINDOWS.map(windowView),
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

export function quotaLatestOf(filters: Filters): QuotaMeter[] {
  return METERS.filter(
    (meter) =>
      (filters.agent === null || meter.agent === filters.agent) &&
      (filters.profile === null || meter.profile === filters.profile),
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
        return { agent: meter.agent, profile: meter.profile, window: seed.window, points, resets };
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
