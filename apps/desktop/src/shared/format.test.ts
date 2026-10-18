import { describe, expect, it } from "vitest";
import {
  type Fx,
  formatClock,
  formatDay,
  formatDuration,
  formatHour,
  formatMoney,
  formatPercent,
  formatRelativeReset,
  formatTokens,
} from "./format";

const ecb = (offline: boolean): Fx => ({
  kind: "ecb",
  offline,
  rates: [
    { date: "2026-03-02", eurPerUsd: 0.9 },
    { date: "2026-03-03", eurPerUsd: 0.8 },
  ],
});

describe("formatMoney", () => {
  it("formats USD micros as exact dollars", () => {
    expect(formatMoney(1_234_560_000, "2026-03-04", "USD", ecb(false))).toEqual({
      approximate: false,
      text: "$1,234.56",
    });
  });

  it("formats zero without a sign", () => {
    expect(formatMoney(0, "2026-03-04", "USD", ecb(false)).text).toBe("$0.00");
    expect(formatMoney(-1, "2026-03-04", "USD", ecb(false)).text).toBe("$0.00");
  });

  it("rounds half away from zero on cents", () => {
    expect(formatMoney(5_000, "2026-03-04", "USD", ecb(false)).text).toBe("$0.01");
    expect(formatMoney(4_999, "2026-03-04", "USD", ecb(false)).text).toBe("$0.00");
    expect(formatMoney(-5_000, "2026-03-04", "USD", ecb(false)).text).toBe("-$0.01");
  });

  it("formats negative amounts", () => {
    expect(formatMoney(-2_500_000, "2026-03-04", "USD", ecb(false)).text).toBe("-$2.50");
  });

  it("converts to EUR with the latest rate dated strictly before the event date", () => {
    // 2026-03-03 must use the 03-02 rate (0.9), not its own 03-03 rate.
    expect(formatMoney(10_000_000, "2026-03-03", "EUR", ecb(false))).toEqual({
      approximate: false,
      text: "€9.00",
    });
    expect(formatMoney(10_000_000, "2026-03-04", "EUR", ecb(false)).text).toBe("€8.00");
  });

  it("uses a fixed rate exactly", () => {
    expect(
      formatMoney(10_000_000, "2026-03-04", "EUR", { kind: "fixed", eurPerUsd: 0.92 }),
    ).toEqual({ approximate: false, text: "€9.20" });
  });

  it("flags the last known rate as approximate while offline", () => {
    expect(formatMoney(10_000_000, "2026-03-09", "EUR", ecb(true))).toEqual({
      approximate: true,
      reason: "offline",
      text: "€8.00",
    });
  });

  it("does not flag older rates while offline", () => {
    expect(formatMoney(10_000_000, "2026-03-03", "EUR", ecb(true)).approximate).toBe(false);
  });

  it("flags events before the first rate", () => {
    expect(formatMoney(10_000_000, "2026-01-01", "EUR", ecb(false))).toEqual({
      approximate: true,
      reason: "before-first-rate",
      text: "€9.00",
    });
  });

  it("reports a missing rate table", () => {
    expect(
      formatMoney(10_000_000, "2026-03-04", "EUR", { kind: "ecb", rates: [], offline: false }),
    ).toEqual({ approximate: true, reason: "no-rate", text: "\u2014" });
  });
});

describe("formatTokens", () => {
  it.each([
    [0, "0"],
    [999, "999"],
    [1_000, "1K"],
    [1_234, "1.2K"],
    [999_949, "999.9K"],
    [999_950, "1M"],
    [3_400_000, "3.4M"],
    [2_500_000_000, "2.5B"],
    [-1_500, "-1.5K"],
  ])("formats %d as %s", (count, text) => {
    expect(formatTokens(count)).toBe(text);
  });
});

describe("formatPercent and formatDuration", () => {
  it("formats fractions", () => {
    expect(formatPercent(0.734)).toBe("73%");
    expect(formatPercent(0.0734, 1)).toBe("7.3%");
    expect(formatPercent(Number.NaN)).toBe("\u2014");
  });

  it.each([
    [0, "0s"],
    [45, "45s"],
    [3_600, "1h"],
    [8_100, "2h 15m"],
    [8_145, "2h 15m"],
    [93_600, "1d 2h"],
    [86_400 + 300, "1d"],
  ])("formats %d seconds as %s", (seconds, text) => {
    expect(formatDuration(seconds)).toBe(text);
  });

  it("formats reset countdowns", () => {
    const now = 1_000_000;
    expect(formatRelativeReset(now + 8_130_000, now)).toBe("resets in 2h 15m");
    expect(formatRelativeReset(now + 30_000, now)).toBe("resets in <1m");
    expect(formatRelativeReset(now - 1, now)).toBe("resets now");
  });
});

describe("time zone bucketing (D8)", () => {
  const paris = "Europe/Paris";

  it("buckets days in the given zone, not UTC", () => {
    const instant = Date.parse("2026-06-15T22:30:00Z");
    expect(formatDay(instant, "UTC")).toBe("2026-06-15");
    expect(formatDay(instant, paris)).toBe("2026-06-16");
    expect(formatDay(instant, "America/Los_Angeles")).toBe("2026-06-15");
  });

  it("splits the 23-hour spring-forward day at local midnight", () => {
    // 2026-03-29: CET (+1) becomes CEST (+2) at 01:00 UTC.
    expect(formatDay(Date.parse("2026-03-28T22:59:59Z"), paris)).toBe("2026-03-28");
    expect(formatDay(Date.parse("2026-03-28T23:00:00Z"), paris)).toBe("2026-03-29");
    expect(formatDay(Date.parse("2026-03-29T21:59:59Z"), paris)).toBe("2026-03-29");
    expect(formatDay(Date.parse("2026-03-29T22:00:00Z"), paris)).toBe("2026-03-30");
  });

  it("skips the non-existent local hour", () => {
    expect(formatHour(Date.parse("2026-03-29T00:59:00Z"), paris)).toBe("01:00");
    expect(formatHour(Date.parse("2026-03-29T01:00:00Z"), paris)).toBe("03:00");
  });

  it("repeats the local hour on the fall-back day", () => {
    // 2026-10-25: CEST (+2) becomes CET (+1) at 01:00 UTC.
    expect(formatHour(Date.parse("2026-10-25T00:30:00Z"), paris)).toBe("02:00");
    expect(formatHour(Date.parse("2026-10-25T01:30:00Z"), paris)).toBe("02:00");
  });

  it("formats wall-clock time", () => {
    expect(formatClock(Date.parse("2026-06-15T13:40:00Z"), paris)).toBe("15:40");
    expect(formatClock(Date.parse("2026-06-15T00:05:00Z"), "UTC")).toBe("00:05");
  });

  it("rejects an invalid zone instead of falling back to the system zone", () => {
    expect(() => formatDay(0, "Not/AZone")).toThrow(RangeError);
  });
});
