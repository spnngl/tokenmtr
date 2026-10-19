import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { etaText, QuotaMeters, windowLabel } from "./quota-meters";
import type { QuotaMeter, QuotaWindow } from "./schemas";

afterEach(cleanup);

const FIVE_HOUR: QuotaWindow = {
  window: "five_hour",
  used_percent: 40,
  resets_at: "2026-06-15T14:00:00Z",
  resets_in_secs: 7200,
  eta: { kind: "wont_hit" },
  spend: null,
};

const EXTRA_USAGE: QuotaWindow = {
  window: "extra_usage",
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

function meter(windows: readonly QuotaWindow[]): QuotaMeter {
  return {
    agent: "claude",
    profile: "default",
    fetched_at: "2026-06-15T12:00:00Z",
    age_secs: 60,
    status: "ok",
    account: null,
    windows: [...windows],
  };
}

const NOW = Date.parse("2026-06-15T12:00:00Z");

describe("QuotaMeters", () => {
  it("shows the monthly extra-usage meter with spend, cap and the danger level", () => {
    render(<QuotaMeters meters={[meter([FIVE_HOUR, EXTRA_USAGE])]} timeZone="UTC" now={NOW} />);
    const extra = screen.getByRole("meter", { name: "Extra usage" });
    expect(within(extra).getByText("93%")).toBeTruthy();
    expect(within(extra).getByText(/\$46\.25/)).toBeTruthy();
    expect(within(extra).getByText(/\$50\.00/)).toBeTruthy();
    expect(within(extra).getByText(/this month/)).toBeTruthy();
    expect(within(extra).queryByText(/resets/i)).toBeNull();
    // 92.5% is past the red threshold, like any other window.
    expect(extra.querySelector("svg.text-danger")).not.toBeNull();
  });

  it("has no extra-usage meter when the account has extra usage off", () => {
    render(<QuotaMeters meters={[meter([FIVE_HOUR])]} timeZone="UTC" now={NOW} />);
    expect(screen.getByRole("meter", { name: "5-hour window" })).toBeTruthy();
    expect(screen.queryByRole("meter", { name: "Extra usage" })).toBeNull();
  });
});

describe("windowLabel", () => {
  it("names known windows and title-cases the others", () => {
    expect(windowLabel("five_hour")).toBe("5-hour window");
    expect(windowLabel("seven_day")).toBe("7-day window");
    expect(windowLabel("extra_usage")).toBe("Extra usage");
    expect(windowLabel("seven_day_opus")).toBe("Seven day opus");
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
