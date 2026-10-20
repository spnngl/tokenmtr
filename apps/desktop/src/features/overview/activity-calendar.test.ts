import { describe, expect, it } from "vitest";
import type { ActivityDay } from "../../shared/schemas";
import { levelsOf, monthsOf, streaksOf, weeksOf, weeksThatFit } from "./activity-calendar";

function day(date: string, total_tokens: number, sessions: number): ActivityDay {
  return { day: date, total_tokens, sessions };
}

describe("activity calendar", () => {
  it("cuts the days in week columns", () => {
    const days = Array.from({ length: 14 }, (_, i) =>
      day(`2026-03-${String(i + 2).padStart(2, "0")}`, i, 0),
    );
    const weeks = weeksOf(days);
    expect(weeks).toHaveLength(2);
    expect(weeks[1]?.[0]?.day).toBe("2026-03-09");
  });

  it("shows the weeks that fit, never fewer than a year nor more than there are", () => {
    expect(weeksThatFit(200, 105)).toBe(53);
    expect(weeksThatFit(28 + 80 * 14 - 3, 105)).toBe(80);
    expect(weeksThatFit(4000, 105)).toBe(105);
  });

  it("keeps idle days at level 0 and spreads active days over four levels", () => {
    const levelOf = levelsOf([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    expect(levelOf(0)).toBe(0);
    expect(levelOf(1)).toBe(1);
    expect(levelOf(8)).toBe(4);
    expect(levelsOf([0, 0])(0)).toBe(0);
  });

  it("finds the streaks and the busiest day; an idle today keeps the current streak", () => {
    const days = [
      day("2026-03-01", 5, 1),
      day("2026-03-02", 9, 1),
      day("2026-03-03", 7, 1),
      day("2026-03-04", 0, 0),
      day("2026-03-05", 9, 2),
      day("2026-03-06", 1, 1),
      day("2026-03-07", 0, 0),
    ];
    expect(streaksOf(days, "tokens")).toEqual({ current: 2, longest: 3, busiest: days[1] });
    expect(streaksOf(days, "sessions").busiest).toBe(days[4]);
    expect(streaksOf([...days, day("2026-03-08", 0, 0)], "tokens").current).toBe(0);
    expect(streaksOf([day("2026-03-01", 0, 0)], "tokens")).toEqual({
      current: 0,
      longest: 0,
      busiest: undefined,
    });
  });

  it("sums the months for the table", () => {
    const months = monthsOf([
      day("2026-02-28", 10, 1),
      day("2026-03-01", 0, 0),
      day("2026-03-02", 5, 2),
    ]);
    expect(months).toEqual([
      { month: "2026-02", tokens: 10, sessions: 1, activeDays: 1 },
      { month: "2026-03", tokens: 5, sessions: 2, activeDays: 1 },
    ]);
  });
});
