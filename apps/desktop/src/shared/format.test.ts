import { describe, expect, it } from "vitest";
import {
  formatClock,
  formatCount,
  formatCurrencyMicros,
  formatDateTime,
  formatDay,
  formatDuration,
  formatPercent,
  formatRelativeReset,
  formatRoiMultiple,
  formatTokens,
} from "./format";

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

describe("time zone bucketing", () => {
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
    expect(formatClock(Date.parse("2026-03-29T00:59:00Z"), paris)).toBe("01:59");
    expect(formatClock(Date.parse("2026-03-29T01:00:00Z"), paris)).toBe("03:00");
  });

  it("repeats the local hour on the fall-back day", () => {
    // 2026-10-25: CEST (+2) becomes CET (+1) at 01:00 UTC.
    expect(formatClock(Date.parse("2026-10-25T00:30:00Z"), paris)).toBe("02:30");
    expect(formatClock(Date.parse("2026-10-25T01:30:00Z"), paris)).toBe("02:30");
  });

  it("formats wall-clock time", () => {
    expect(formatClock(Date.parse("2026-06-15T13:40:00Z"), paris)).toBe("15:40");
    expect(formatClock(Date.parse("2026-06-15T00:05:00Z"), "UTC")).toBe("00:05");
  });

  it("rejects an invalid zone instead of falling back to the system zone", () => {
    expect(() => formatDay(0, "Not/AZone")).toThrow(RangeError);
  });
});

describe("formatCurrencyMicros", () => {
  it("formats converted amounts without applying a rate", () => {
    expect(formatCurrencyMicros(1_234_560_000, "USD")).toBe("$1,234.56");
    expect(formatCurrencyMicros(1_234_560_000, "EUR")).toBe("€1,234.56");
  });

  it("rounds half away from zero and never prints a negative zero", () => {
    expect(formatCurrencyMicros(5_000, "USD")).toBe("$0.01");
    expect(formatCurrencyMicros(4_999, "USD")).toBe("$0.00");
    expect(formatCurrencyMicros(-5_000, "USD")).toBe("-$0.01");
    expect(formatCurrencyMicros(-4_000, "USD")).toBe("$0.00");
    expect(formatCurrencyMicros(-2_500_000, "EUR")).toBe("-€2.50");
  });
});

describe("formatCount", () => {
  it("groups thousands and dashes a non-number", () => {
    expect(formatCount(12_345)).toBe("12,345");
    expect(formatCount(0)).toBe("0");
    expect(formatCount(Number.NaN)).toBe("\u2014");
  });
});

describe("formatRoiMultiple", () => {
  it("shows one decimal and a dash without a subscription", () => {
    expect(formatRoiMultiple(4)).toBe("4.0×");
    expect(formatRoiMultiple(0.46)).toBe("0.5×");
    expect(formatRoiMultiple(null)).toBe("—");
  });
});

describe("formatDateTime", () => {
  it("uses the given zone for both the day and the clock", () => {
    const instant = Date.parse("2026-06-15T23:30:00Z");
    expect(formatDateTime(instant, "Europe/Paris")).toBe("2026-06-16 01:30");
    expect(formatDateTime(instant, "America/New_York")).toBe("2026-06-15 19:30");
  });
});
