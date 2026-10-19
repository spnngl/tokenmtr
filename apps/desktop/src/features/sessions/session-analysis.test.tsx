import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { type Harness, renderApp } from "../../app/app-harness";
import { axeViolations } from "../../shared/axe";
import type { SessionPoint } from "../../shared/schemas";
import {
  cacheHitBins,
  costBucketLabel,
  histogramByModel,
  tokenMixValues,
  toolErrorRanking,
} from "./analysis-model";

afterEach(cleanup);

const FIGURES = [
  "Sessions by API value",
  "API value against duration",
  "Cache-hit ratio",
  "Token mix of the costliest sessions",
  "Sessions per day",
] as const;

async function openAnalysis(options: Harness = {}) {
  const user = userEvent.setup();
  const view = await renderApp("/sessions", options);
  await user.click(await screen.findByRole("tab", { name: "Analysis" }));
  return { user, ...view };
}

function point(overrides: Partial<SessionPoint>): SessionPoint {
  return {
    agent: "claude",
    session: "s",
    started_at: "2026-06-15T10:00:00Z",
    duration_secs: 60,
    api_value_micros: 0,
    paid_micros: 0,
    cache_hit_percent: null,
    tokens: {
      input_uncached: 0,
      cache_read: 0,
      cache_write_5m: 0,
      cache_write_1h: 0,
      output: 0,
      reasoning: 0,
    },
    tool_calls: 0,
    tool_errors: 0,
    ...overrides,
  };
}

describe("session analysis model", () => {
  it("labels the first, inner and open cost buckets", () => {
    expect(
      costBucketLabel({ min_micros: 0, max_micros: 10_000, sessions: 1, models: [] }, "USD"),
    ).toBe("< $0.01");
    expect(
      costBucketLabel({ min_micros: 10_000, max_micros: 20_000, sessions: 1, models: [] }, "USD"),
    ).toBe("$0.01–$0.02");
    expect(
      costBucketLabel(
        { min_micros: 100_000_000, max_micros: null, sessions: 1, models: [] },
        "USD",
      ),
    ).toBe("≥ $100.00");
  });

  it("stacks the histogram by dominant model, top five then Other", () => {
    const models = ["a", "b", "c", "d", "e", "f", "g"].map((model, index) => ({
      model,
      sessions: 10 - index,
    }));
    const { series, rows } = histogramByModel({
      currency: "USD",
      total: 0,
      rows: [],
      histogram: [
        { min_micros: 0, max_micros: 10_000, sessions: 49, models },
        { min_micros: 10_000, max_micros: null, sessions: 0, models: [] },
      ],
      days: [],
      starts: [],
    });
    expect(series).toEqual(["a", "b", "c", "d", "e", "Other"]);
    expect(rows[0]?.values).toEqual([10, 9, 8, 7, 6, 9]);
    expect(rows[1]?.values).toEqual([0, 0, 0, 0, 0, 0]);
  });

  it("bins cache-hit ratios by ten points and skips sessions without input", () => {
    const bins = cacheHitBins([
      point({ cache_hit_percent: 0 }),
      point({ cache_hit_percent: 95 }),
      point({ cache_hit_percent: 100 }),
      point({ cache_hit_percent: null }),
    ]);
    expect(bins).toHaveLength(10);
    expect(bins[0]).toEqual({ label: "0–10%", value: 1 });
    expect(bins[9]).toEqual({ label: "90–100%", value: 2 });
  });

  it("ranks failed tool calls by rate, then count", () => {
    const ranked = toolErrorRanking(
      [
        point({ session: "a", tool_calls: 10, tool_errors: 1 }),
        point({ session: "b", tool_calls: 2, tool_errors: 1 }),
        point({ session: "c", tool_calls: 4, tool_errors: 2 }),
        point({ session: "d", tool_calls: 5, tool_errors: 0 }),
      ],
      10,
    );
    expect(ranked.map((rank) => rank.row.session)).toEqual(["c", "b", "a"]);
  });

  it("splits reasoning out of output and sums cache writes", () => {
    expect(
      tokenMixValues({
        input_uncached: 1,
        cache_read: 2,
        cache_write_5m: 3,
        cache_write_1h: 4,
        output: 10,
        reasoning: 6,
      }),
    ).toEqual([1, 2, 7, 4, 6]);
  });
});

describe("sessions Analysis tab", () => {
  it("draws every chart within the point budget and is accessible", async () => {
    const { container } = await openAnalysis();
    for (const name of FIGURES) {
      const figure = await screen.findByRole("figure", { name });
      const points = Number(figure.getAttribute("data-points"));
      expect(points).toBeGreaterThan(0);
      expect(points).toBeLessThanOrEqual(400);
    }
    expect(screen.getByRole("table", { name: "Session starts by weekday and hour" })).toBeTruthy();
    const top = screen.getByRole("list", { name: "Top sessions by API value" });
    expect(within(top).getAllByRole("link").length).toBeGreaterThan(0);
    expect(screen.getByRole("list", { name: "Highest tool error rate" })).toBeTruthy();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("shows each chart as a table on demand", async () => {
    const { user } = await openAnalysis();
    for (const name of FIGURES) {
      const figure = await screen.findByRole("figure", { name });
      const details = figure.querySelector("details");
      expect(details?.open).toBe(false);
      await user.click(within(figure).getByText("View as table"));
      expect(details?.open).toBe(true);
      expect(within(figure).getByRole("table")).toBeTruthy();
    }
  });

  it("opens a session from a scatter dot", async () => {
    const { user, router, container } = await openAnalysis();
    await screen.findByRole("figure", { name: "API value against duration" });
    await waitFor(() => expect(container.querySelector(".recharts-scatter-symbol")).not.toBeNull());
    const dot = container.querySelector(".recharts-scatter-symbol");
    if (dot === null) {
      throw new Error("no scatter dot");
    }
    await user.click(dot);
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/sessions\/[^/]+\/./));
  });

  it("says so when no session matches", async () => {
    await openAnalysis({
      wrap: (ds) => ({
        ...ds,
        sessionAnalytics: async (filters) => ({
          ...(await ds.sessionAnalytics(filters)),
          total: 0,
          rows: [],
        }),
      }),
    });
    expect(await screen.findByText("No sessions")).toBeTruthy();
  });

  it("offers Retry when the analysis fails", async () => {
    await openAnalysis({
      wrap: (ds) => ({
        ...ds,
        sessionAnalytics: () => Promise.reject(new Error("store unavailable")),
      }),
    });
    expect(await screen.findByRole("button", { name: "Retry" })).toBeTruthy();
  });
});
