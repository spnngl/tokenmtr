import { afterEach, describe, expect, it } from "vitest";
import { readStoredRange, storeRange } from "./stored-range";

afterEach(() => {
  localStorage.clear();
});

describe("stored range", () => {
  it("round-trips presets and custom ranges", () => {
    expect(readStoredRange()).toBeNull();
    storeRange({ kind: "preset", preset: "90d" });
    expect(readStoredRange()).toEqual({ kind: "preset", preset: "90d" });
    storeRange({ kind: "custom", from: "2026-01-01", to: "2026-01-31" });
    expect(readStoredRange()).toEqual({ kind: "custom", from: "2026-01-01", to: "2026-01-31" });
  });

  it.each([
    "not json",
    '"30d"',
    '{"kind":"preset","preset":"1y"}',
    '{"kind":"custom","from":"2026-02-30","to":"2026-03-01"}',
    '{"kind":"custom","from":"2026-03-02","to":"2026-03-01"}',
  ])("ignores invalid data %s", (raw) => {
    localStorage.setItem("filters.range", raw);
    expect(readStoredRange()).toBeNull();
  });
});
