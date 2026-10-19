import type { Range, RangePreset } from "../../shared/filters";
import type { IsoDate } from "../../shared/format";
import type { Granularity } from "../../shared/schemas";

/** The billing chart's own range, independent of the global filter unless `filter`. */
export const RANGE_CHOICES = [
  { value: "today", label: "Today" },
  { value: "30d", label: "30 days" },
  { value: "90d", label: "90 days" },
  { value: "1y", label: "1 year" },
  { value: "filter", label: "Filter range" },
] as const;
export type RangeChoice = (typeof RANGE_CHOICES)[number]["value"];

export const GRANULARITY_LABELS: Readonly<Record<Granularity, string>> = {
  hourly: "Hourly",
  daily: "Daily",
  monthly: "Monthly",
};

const DAY_MS = 86_400_000;
const PRESET_DAYS: Readonly<Record<RangePreset, number>> = {
  today: 1,
  "7d": 7,
  "30d": 30,
  "90d": 90,
};
/** Longest range drawn per day: keeps a chart near the ~400-point budget. */
const MAX_DAILY_DAYS = 400;
/** Longest range that defaults to daily buckets (about a quarter). */
const DEFAULT_DAILY_DAYS = 92;
const YEAR_DAYS = 365;

/** `date` moved by `days` calendar days. */
export function shiftDate(date: IsoDate, days: number): IsoDate {
  return new Date(Date.parse(`${date}T00:00:00Z`) + days * DAY_MS).toISOString().slice(0, 10);
}

/** Calendar days a range covers, both ends included. */
export function rangeDays(range: Range): number {
  if (range.kind === "preset") {
    return PRESET_DAYS[range.preset];
  }
  return Math.round((Date.parse(range.to) - Date.parse(range.from)) / DAY_MS) + 1;
}

/** The choice matching the global range; other presets and custom ranges are "Filter range". */
export function choiceOf(range: Range): RangeChoice {
  if (range.kind === "preset" && range.preset !== "7d") {
    return range.preset;
  }
  return "filter";
}

/**
 * The range a choice stands for. `dayOffset` (0 or less) moves "Today" back by days; "1 year" is
 * the 365 days ending `today`.
 */
export function rangeOf(
  choice: RangeChoice,
  dayOffset: number,
  globalRange: Range,
  today: IsoDate,
): Range {
  switch (choice) {
    case "today": {
      if (dayOffset === 0) {
        return { kind: "preset", preset: "today" };
      }
      const day = shiftDate(today, dayOffset);
      return { kind: "custom", from: day, to: day };
    }
    case "30d":
    case "90d":
      return { kind: "preset", preset: choice };
    case "1y":
      return { kind: "custom", from: shiftDate(today, 1 - YEAR_DAYS), to: today };
    case "filter":
      return globalRange;
    default:
      return choice satisfies never;
  }
}

/** Bucket sizes worth offering: hours for a single day, days up to the point budget. */
export function granularityOptions(days: number): Granularity[] {
  const options: Granularity[] = [];
  if (days === 1) {
    options.push("hourly");
  }
  if (days <= MAX_DAILY_DAYS) {
    options.push("daily");
  }
  options.push("monthly");
  return options;
}

/** Hours for one day, days up to a quarter, months beyond. */
export function defaultGranularity(days: number): Granularity {
  if (days === 1) {
    return "hourly";
  }
  return days <= DEFAULT_DAILY_DAYS ? "daily" : "monthly";
}
