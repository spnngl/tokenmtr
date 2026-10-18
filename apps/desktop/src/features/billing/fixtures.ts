import type { Budget } from "../../shared/schemas";
import { micros, usage } from "../common/fixtures";
import type { BillingData } from "./model";

const MONTHS = ["2026-03-01", "2026-04-01", "2026-05-01"] as const;

export const BUDGET_OVER: Budget = {
  id: "monthly-paid",
  period: "month",
  measure: "paid",
  currency: "USD",
  period_start: "2026-05-01",
  period_end: "2026-05-31",
  limit_micros: micros(100),
  spent_micros: micros(128),
  percent: 128,
  projected_micros: micros(190),
  projected_percent: 190,
};

export const BUDGET_PROJECTED: Budget = {
  id: "weekly-api",
  period: "week",
  measure: "api_value",
  currency: "USD",
  period_start: "2026-05-18",
  period_end: "2026-05-24",
  limit_micros: micros(200),
  spent_micros: micros(120),
  percent: 60,
  projected_micros: micros(240),
  projected_percent: 120,
};

/** A subscription, a pay-as-you-go tail, three months, budgets over 100%, three advisor outcomes. */
export const BILLING_DATA: BillingData = {
  report: {
    currency: "USD",
    total: usage({
      events: 4100,
      total_tokens: 90_000_000,
      api_value_micros: micros(640),
      api_value_usd_micros: micros(640),
      paid_micros: micros(260),
      subscription_value_micros: micros(560),
      subscription_fee_micros: micros(200),
    }),
    api: usage({
      events: 300,
      api_value_micros: micros(60),
      api_value_usd_micros: micros(60),
      paid_micros: micros(60),
      approximate: true,
    }),
    subscriptions: [
      {
        id: "openai-pro",
        provider: "openai",
        plan: "pro",
        fee_micros: micros(20),
        fee_currency: "USD",
        period: "monthly",
        start: "2026-01-01",
        end: null,
        usage: usage({
          events: 3800,
          subscription_value_micros: micros(560),
          subscription_fee_micros: micros(200),
          api_value_micros: micros(560),
        }),
        roi: 2.8,
      },
      {
        id: "old-team",
        provider: "anthropic",
        plan: "team",
        fee_micros: micros(300),
        fee_currency: "EUR",
        period: "yearly",
        start: "2025-06-01",
        end: "2026-02-01",
        usage: usage({}),
        roi: null,
      },
    ],
    months: MONTHS.map((month, index) => ({
      month,
      ...usage({
        events: 1000 + index * 100,
        api_value_micros: micros(180 + index * 40),
        api_value_usd_micros: micros(180 + index * 40),
        paid_micros: micros(70 + index * 10),
        subscription_fee_micros: micros(20),
      }),
      roi: index === 0 ? null : 2 + index,
    })),
    roi: 2.8,
  },
  budgets: [BUDGET_OVER, BUDGET_PROJECTED],
  advisor: [
    {
      agent: "claude-code",
      profile: "default",
      current_plan: "max_5x",
      snapshots: 420,
      monthly_api_value_usd_micros: micros(640),
      advice: {
        advice: "recommend",
        verdict: { verdict: "upgrade", plan: "max_20x" },
        current: "max_5x",
        headroom: 4,
        fits: [
          {
            plan: "max_5x",
            monthly_fee: micros(100),
            five_hour_peak: 96,
            seven_day_peak: 88,
            fits: false,
          },
          {
            plan: "max_20x",
            monthly_fee: micros(200),
            five_hour_peak: 24,
            seven_day_peak: 22,
            fits: true,
          },
        ],
      },
    },
    {
      agent: "codex",
      profile: "default",
      current_plan: null,
      snapshots: 6,
      monthly_api_value_usd_micros: 0,
      advice: { advice: "not_enough_data", span_days: 5 },
    },
    {
      agent: "gemini",
      profile: "default",
      current_plan: null,
      snapshots: 0,
      monthly_api_value_usd_micros: 0,
      advice: { advice: "not_applicable" },
    },
  ],
};
