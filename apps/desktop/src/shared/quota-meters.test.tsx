import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { etaText, QuotaMeters } from "./quota-meters";
import type { QuotaMeter, QuotaWindow } from "./schemas";

afterEach(cleanup);

const FIVE_HOUR: QuotaWindow = {
  window: "five_hour",
  label: "5-hour",
  used_percent: 40,
  resets_at: "2026-06-15T14:00:00Z",
  resets_in_secs: 7200,
  eta: { kind: "wont_hit" },
  spend: null,
};

const FABLE: QuotaWindow = {
  window: "weekly_scoped:fable",
  label: "Weekly · Fable",
  used_percent: 75,
  resets_at: "2026-06-18T12:00:00Z",
  resets_in_secs: 259_200,
  eta: { kind: "not_enough_data" },
  spend: null,
};

const EXTRA_USAGE: QuotaWindow = {
  window: "extra_usage",
  label: "Extra usage",
  used_percent: 92.5,
  resets_at: null,
  resets_in_secs: null,
  eta: { kind: "not_enough_data" },
  spend: {
    currency: "USD",
    spent_micros: 46_250_000,
    limit_micros: 50_000_000,
    approximate: false,
  },
};

function meter(windows: readonly QuotaWindow[], profile = "default"): QuotaMeter {
  return {
    agent: "claude",
    profile,
    fetched_at: "2026-06-15T12:00:00Z",
    age_secs: 60,
    status: "ok",
    account: null,
    windows: [...windows],
  };
}

const NOW = Date.parse("2026-06-15T12:00:00Z");

describe("QuotaMeters", () => {
  it("draws one donut per window named by the backend label, with percent and reset", () => {
    render(<QuotaMeters meters={[meter([FIVE_HOUR, FABLE])]} timeZone="UTC" now={NOW} />);
    const session = screen.getByRole("meter", { name: "5-hour" });
    expect(session.getAttribute("aria-valuetext")).toBe("40% used, resets in 2h");
    expect(within(session).getByText("40%")).toBeTruthy();
    expect(within(session).getByText("Will not reach 100% before reset")).toBeTruthy();
    const fable = screen.getByRole("meter", { name: "Weekly · Fable" });
    // 75% is past the amber threshold: an icon, not colour alone.
    expect(fable.querySelector("svg.text-warning")).not.toBeNull();
  });

  it("shows the monthly extra-usage donut with spend, cap and the danger level", () => {
    render(<QuotaMeters meters={[meter([FIVE_HOUR, EXTRA_USAGE])]} timeZone="UTC" now={NOW} />);
    const extra = screen.getByRole("meter", { name: "Extra usage" });
    expect(within(extra).getByText("93%")).toBeTruthy();
    expect(within(extra).getByText(/\$46\.25/)).toBeTruthy();
    expect(within(extra).getByText(/\$50\.00/)).toBeTruthy();
    expect(within(extra).getByText(/this month/)).toBeTruthy();
    expect(within(extra).queryByText(/resets/i)).toBeNull();
    expect(extra.querySelector("svg.text-danger")).not.toBeNull();
  });

  it("groups donuts per profile and shows a profile's problem in its own group", () => {
    const expired: QuotaMeter = { ...meter([], "work"), status: "token_expired" };
    render(<QuotaMeters meters={[meter([FIVE_HOUR]), expired]} timeZone="UTC" now={NOW} />);
    const main = screen.getByRole("region", { name: "claude / default" });
    expect(within(main).getByRole("meter", { name: "5-hour" })).toBeTruthy();
    const work = screen.getByRole("region", { name: "claude / work" });
    expect(within(work).queryByRole("meter")).toBeNull();
    expect(within(work).getByRole("status").textContent).toMatch(/Token expired/);
  });
});

describe("etaText", () => {
  it("shows the clock time in the configured zone", () => {
    expect(etaText({ kind: "will_hit", at: "2026-06-15T13:40:00Z" }, "Europe/Paris")).toBe(
      "100% at 15:40 at this pace",
    );
  });

  it("says so when the window will not fill, and stays silent without data", () => {
    expect(etaText({ kind: "wont_hit" }, "UTC")).toBe("Will not reach 100% before reset");
    expect(etaText({ kind: "not_enough_data" }, "UTC")).toBeNull();
  });
});
