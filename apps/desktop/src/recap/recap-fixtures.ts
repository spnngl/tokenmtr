import { ALL_TAB_ID, type RecapModel, type RecapView } from "../shared/recap-model";

/** Placeholder data for tests only; the app never imports this. */

export const NOW_MS = Date.parse("2026-06-15T12:00:00Z");

export const FULL_VIEW: RecapView = {
  quota: [
    {
      id: "5h",
      label: "5h window",
      usedFraction: 0.74,
      resetsAtMs: NOW_MS + 8_100_000,
      projectedFullAtMs: Date.parse("2026-06-15T13:40:00Z"),
    },
    {
      id: "7d",
      label: "7-day window",
      usedFraction: 0.93,
      resetsAtMs: NOW_MS + 3 * 86_400_000,
      projectedFullAtMs: null,
    },
  ],
  money: {
    paidUsdMicros: 12_340_000,
    apiValueUsdMicros: 48_760_000,
    roi: 3.9,
    projectedUsdMicros: 61_000_000,
  },
  budgets: [
    {
      id: "monthly",
      label: "Monthly budget",
      usedFraction: 0.5,
      spentUsdMicros: 25_000_000,
      projectedUsdMicros: 50_000_000,
    },
  ],
  tokens: { total: 3_400_000, cachedInputFraction: 0.82, output: 120_000 },
  trendUsdMicros: [1_000_000, 4_000_000, 2_000_000, 8_000_000, 5_000_000, 6_000_000, 9_000_000],
  live: { tokensPerMinute: 12_500, apiValueUsdMicrosPerHour: 3_200_000, activeSessions: 2 },
  activity: { topModel: "claude-opus-4", lastActivityAtMs: NOW_MS - 120_000 },
};

/** An agent with every capability off: no quota, budget or live data. */
export const BARE_VIEW: RecapView = {
  quota: [],
  money: { paidUsdMicros: 0, apiValueUsdMicros: 0, roi: null, projectedUsdMicros: null },
  budgets: [],
  tokens: { total: 0, cachedInputFraction: 0, output: 0 },
  trendUsdMicros: [0, 0, 0, 0, 0, 0, 0],
  live: null,
  activity: { topModel: null, lastActivityAtMs: null },
};

export const MODEL: RecapModel = {
  period: "today",
  nowMs: NOW_MS,
  asOf: "2026-06-15",
  health: "ok",
  tabs: [
    { id: ALL_TAB_ID, label: "All", view: FULL_VIEW },
    { id: "claude", label: "Claude", view: FULL_VIEW },
    { id: "pi", label: "pi", view: BARE_VIEW },
  ],
};
