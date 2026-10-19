import { describe, expect, it } from "vitest";
import { usage } from "../../shared/fixtures";
import { chartModels, costPerMillion, foldModels, type ModelTotals } from "./model";

function model(name: string, api: number, overrides: Partial<ModelTotals> = {}): ModelTotals {
  return { ...usage({ api_value_micros: api, total_tokens: 1000 }), model: name, ...overrides };
}

const ROWS = ["a", "b", "c", "d", "e", "f", "g"].map((name, index) =>
  model(name, (7 - index) * 1000, { cache_read: 10, input_uncached: 30 }),
);

describe("per-model chart series", () => {
  it("keeps the top five models and folds the rest into Other", () => {
    const top = chartModels(ROWS);
    expect(top).toEqual(["a", "b", "c", "d", "e", "Other"]);
    const folded = foldModels(ROWS, top);
    expect(folded.map((row) => row.model)).toEqual(top);
    const other = folded.at(-1);
    expect(other?.api_value_micros).toBe(2000 + 1000);
    expect(other?.cache_read).toBe(20);
    expect(other?.total_tokens).toBe(2000);
  });

  it("has no Other when five models or fewer exist", () => {
    expect(chartModels(ROWS.slice(0, 5))).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("marks a fold approximate when any part is", () => {
    const rows = [...ROWS.slice(0, 6), model("g", 1, { approximate: true })];
    expect(foldModels(rows, chartModels(rows)).at(-1)?.approximate).toBe(true);
  });

  it("prices a million tokens, and nothing without tokens", () => {
    expect(costPerMillion(model("a", 3_000, { total_tokens: 1000 }))).toBe(3_000_000);
    expect(costPerMillion(model("a", 3_000, { total_tokens: 0 }))).toBeNull();
  });
});
