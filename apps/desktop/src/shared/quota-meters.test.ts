import { describe, expect, it } from "vitest";
import { etaText, windowLabel } from "./quota-meters";

describe("windowLabel", () => {
  it("names known windows and title-cases the others", () => {
    expect(windowLabel("five_hour")).toBe("5-hour window");
    expect(windowLabel("seven_day")).toBe("7-day window");
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
