import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type AppConfig, bootstrapConfig } from "./app-context";
import type { DataSource } from "./data-source";
import type { Listen } from "./events";
import { createMockDataSource, isMockScenario } from "./mock";
import { MOCK_TIME_ZONE, NOW_MS } from "./mock/seed";
import { createTauriDataSource } from "./tauri-data-source";

export interface Runtime {
  readonly dataSource: DataSource;
  readonly config: AppConfig;
  /** The shell's pushed events; `undefined` outside Tauri (mock data never changes). */
  readonly listen: Listen | undefined;
}

function insideTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

const tauriListen: Listen = (event, handler) =>
  listen(event, (message) => handler(message.payload));

/**
 * Inside Tauri the real commands with the bootstrap config (`AppProvider` swaps in the settings'
 * zone and currency once they arrive, so the window never waits on the shell). Elsewhere
 * (`vite dev`, screenshots) the seeded mock, with the scenario picked by `?mock=<scenario>` (see
 * `MOCK_SCENARIOS`).
 */
export function createRuntime(search: string): Runtime {
  if (insideTauri()) {
    return {
      dataSource: createTauriDataSource(invoke),
      config: bootstrapConfig,
      listen: tauriListen,
    };
  }
  const requested = new URLSearchParams(search).get("mock");
  return {
    dataSource: createMockDataSource({
      scenario: isMockScenario(requested) ? requested : "populated",
    }),
    config: { ...bootstrapConfig, timeZone: MOCK_TIME_ZONE, now: () => NOW_MS },
    listen: undefined,
  };
}
