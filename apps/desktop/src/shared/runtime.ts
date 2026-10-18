import { invoke } from "@tauri-apps/api/core";
import { type AppConfig, bootstrapConfig } from "./app-context";
import type { DataSource } from "./data-source";
import { createMockDataSource, isMockScenario } from "./mock";
import { MOCK_TIME_ZONE, NOW_MS } from "./mock/seed";
import { createTauriDataSource } from "./tauri-data-source";

export interface Runtime {
  readonly dataSource: DataSource;
  readonly config: AppConfig;
}

function insideTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Inside Tauri the real commands; elsewhere (`vite dev`, screenshots) the seeded mock, with the
 * scenario picked by `?mock=<scenario>` (see `MOCK_SCENARIOS`).
 */
export function createRuntime(search: string): Runtime {
  if (insideTauri()) {
    return { dataSource: createTauriDataSource(invoke), config: bootstrapConfig };
  }
  const requested = new URLSearchParams(search).get("mock");
  return {
    dataSource: createMockDataSource({
      scenario: isMockScenario(requested) ? requested : "populated",
    }),
    config: { ...bootstrapConfig, timeZone: MOCK_TIME_ZONE, now: () => NOW_MS },
  };
}
