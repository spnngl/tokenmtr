import { micros, usage } from "../../shared/fixtures";
import type { Budget, MeasuredBudget } from "../../shared/schemas";
import type { BillingData } from "./model";

const MONTHS = ["2026-03-01", "2026-04-01", "2026-05-01"] as const;

export const BUDGET_OVER: MeasuredBudget = {
  state: "measured",
  id: "monthly-paid",
  period: "month",
  measure: "paid",
  currency: "USD",
  period_start: "2026-05-01",
  period_end: "2026-06-01",
  limit_micros: micros(100),
  spent_micros: micros(128),
  percent: 128,
  projected_micros: micros(190),
  projected_percent: 190,
};

export const BUDGET_PROJECTED: MeasuredBudget = {
  state: "measured",
  id: "weekly-api",
  period: "week",
  measure: "api_value",
  currency: "USD",
  period_start: "2026-05-18",
  period_end: "2026-05-25",
  limit_micros: micros(200),
  spent_micros: micros(120),
  percent: 60,
  projected_micros: micros(240),
  projected_percent: 120,
};

/** An EUR budget before the first FX rate: only its limit is known. */
export const BUDGET_NO_RATE: Budget = {
  state: "no_fx_rate",
  id: "daily-eur",
  period: "day",
  measure: "paid",
  currency: "EUR",
  period_start: "2026-05-20",
  period_end: "2026-05-21",
  limit_micros: micros(20),
};

/**
 * A subscription, a pay-as-you-go tail, Claude extra usage in the last month, three months,
 * budgets over 100%, three advisor outcomes.
 */
export const BILLING_DATA: BillingData = {
  report: {
    currency: "USD",
    total: usage({
      events: 4100,
      total_tokens: 90_000_000,
      api_value_micros: micros(640),
      api_value_usd_micros: micros(640),
      paid_micros: micros(272.35),
      extra_usage_micros: micros(12.35),
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
        paid_micros: micros(70 + index * 10 + (index === 2 ? 12.35 : 0)),
        extra_usage_micros: micros(index === 2 ? 12.35 : 0),
        subscription_fee_micros: micros(20),
      }),
      roi: index === 0 ? null : 2 + index,
    })),
    roi: 2.8,
  },
  budgets: [BUDGET_OVER, BUDGET_PROJECTED, BUDGET_NO_RATE],
  advisor: [
    {
      agent: "claude-code",
      profile: "default",
      current_plan: "max_5x",
      organization: null,
      snapshots: 420,
      monthly_api_value_usd_micros: micros(640),
      advice: {
        advice: "recommend",
        verdict: { verdict: "upgrade", plan: "max_20x" },
        current: "max_5x",
        sizing: { source: "catalog" },
        current_monthly_fee: micros(100),
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
      agent: "claude-code",
      profile: "work",
      current_plan: "team",
      organization: "Wiremind",
      snapshots: 380,
      monthly_api_value_usd_micros: micros(12),
      advice: {
        advice: "recommend",
        verdict: { verdict: "downgrade", plan: "pro" },
        current: "team",
        sizing: { source: "rate_tier", plan: "max_5x" },
        current_monthly_fee: null,
        headroom: 80,
        fits: [
          {
            plan: "pro",
            monthly_fee: micros(20),
            five_hour_peak: 25,
            seven_day_peak: 15,
            fits: true,
          },
        ],
      },
    },
    {
      agent: "claude-code",
      profile: "lab",
      current_plan: "team",
      organization: "Wiremind",
      snapshots: 90,
      monthly_api_value_usd_micros: 0,
      advice: { advice: "unknown_quota" },
    },
    {
      agent: "codex",
      profile: "default",
      current_plan: null,
      organization: null,
      snapshots: 6,
      monthly_api_value_usd_micros: 0,
      advice: { advice: "not_enough_data", span_days: 5 },
    },
    {
      agent: "gemini",
      profile: "default",
      current_plan: null,
      organization: null,
      snapshots: 0,
      monthly_api_value_usd_micros: 0,
      advice: { advice: "no_plan" },
    },
  ],
};
