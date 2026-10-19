import { describe, expect, it } from "vitest";
import { defaultFilters } from "../filters";
import { createMockDataSource } from "./mock-data-source";

const FILTERS = { ...defaultFilters("USD"), range: { kind: "preset", preset: "30d" } } as const;

describe("mock analytics", () => {
  it("bills the Claude subscription fee, so paid differs from API value", async () => {
    const mock = createMockDataSource();
    const daily = await mock.billingSeries(FILTERS, "daily", true);
    expect(daily.points).toHaveLength(30);
    const last = daily.points.at(-1);
    expect(last?.cumulative_paid_micros).not.toBe(last?.cumulative_api_value_micros);
    expect(daily.points.every((p) => (p.agents.claude?.subscription_fee_micros ?? 0) > 0)).toBe(
      true,
    );
    const hourly = await mock.billingSeries(FILTERS, "hourly", false);
    expect(hourly.points).toHaveLength(30 * 24);
    const firstDayFee = hourly.points
      .slice(0, 24)
      .reduce((sum, p) => sum + p.subscription_fee_micros, 0);
    expect(firstDayFee).toBe(daily.points[0]?.subscription_fee_micros);
  });

  it("charts sessions and a timeline that sums to the session", async () => {
    const mock = createMockDataSource();
    const analytics = await mock.sessionAnalytics(FILTERS);
    expect(analytics.rows.length).toBe(analytics.total);
    expect(analytics.histogram.reduce((sum, b) => sum + b.sessions, 0)).toBe(analytics.total);
    expect(analytics.starts).toHaveLength(168);
    const top = analytics.rows[0];
    expect(top).toBeDefined();
    if (top === undefined) {
      return;
    }
    const timeline = await mock.sessionTimeline(top.agent, top.session, "USD");
    expect(timeline.events.at(-1)?.cumulative_api_value_micros).toBe(top.api_value_micros);
  });

  it("answers empty and rejects in the error scenario", async () => {
    const empty = createMockDataSource({ scenario: "empty" });
    const series = await empty.billingSeries(FILTERS, "daily", false);
    expect(series.points.every((p) => p.paid_micros === 0 && p.roi === null)).toBe(true);
    expect((await empty.sessionAnalytics(FILTERS)).total).toBe(0);
    const failing = createMockDataSource({ scenario: "error" });
    await expect(failing.billingSeries(FILTERS, "monthly", false)).rejects.toThrow();
    await expect(failing.sessionAnalytics(FILTERS)).rejects.toThrow();
    await expect(failing.sessionTimeline("claude", "x", "USD")).rejects.toThrow();
  });
});
