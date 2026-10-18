import { micros, usage } from "../common/fixtures";
import type { ModelsData } from "./model";

/** Four models: priced and reported, estimated and divergent, and one unpriced. */
export const MODELS_REPORT: ModelsData = {
  currency: "USD",
  rows: [
    {
      model: "claude-opus-4-5",
      ...usage({
        events: 1840,
        input_uncached: 2_400_000,
        cache_read: 61_000_000,
        cache_write_5m: 3_100_000,
        output: 1_900_000,
        api_value_micros: micros(412.5),
        api_value_usd_micros: micros(412.5),
        paid_micros: micros(120),
        reported_events: 1840,
      }),
      share_percent: 71.4,
      reported_vs_estimated_percent: 1.8,
      divergent: false,
    },
    {
      model: "gpt-5-codex",
      ...usage({
        events: 920,
        input_uncached: 1_100_000,
        cache_read: 9_000_000,
        output: 800_000,
        api_value_micros: micros(143.25),
        api_value_usd_micros: micros(143.25),
        estimated_events: 920,
        approximate: true,
      }),
      share_percent: 24.8,
      reported_vs_estimated_percent: -7.4,
      divergent: true,
    },
    {
      model: "claude-haiku-4-5",
      ...usage({
        events: 310,
        input_uncached: 300_000,
        output: 90_000,
        api_value_micros: micros(21.9),
        api_value_usd_micros: micros(21.9),
        reported_events: 310,
      }),
      share_percent: 3.8,
      reported_vs_estimated_percent: null,
      divergent: false,
    },
    {
      model: "mystery-model-x",
      ...usage({
        events: 12,
        input_uncached: 40_000,
        output: 6_000,
        unpriced_events: 12,
        approximate: true,
      }),
      share_percent: 0,
      reported_vs_estimated_percent: null,
      divergent: false,
    },
  ],
};
