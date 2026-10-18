import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type AppConfig, bootstrapConfig } from "./app-context";
import type { DataSource } from "./data-source";
import type { Listen } from "./events";
import { createMockDataSource, isMockScenario } from "./mock";
import { MOCK_TIME_ZONE, NOW_MS } from "./mock/seed";
import type { Settings } from "./schemas";
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

/** The UI's config from the shell's settings: display zone and currency, and the FX source. */
export function configFromSettings(settings: Settings): AppConfig {
  const { fixed_eur_per_usd: fixed, offline } = settings.fx;
  return {
    ...bootstrapConfig,
    timeZone: settings.display.time_zone,
    defaultCurrency: settings.display.currency,
    fx: fixed === null ? { kind: "ecb", rates: [], offline } : { kind: "fixed", eurPerUsd: fixed },
  };
}

const tauriListen: Listen = (event, handler) =>
  listen(event, (message) => handler(message.payload));

/**
 * Inside Tauri the real commands and the config the shell reports (the bootstrap config when the
 * shell cannot answer yet, for example a daemon-less first launch with a broken config: the app
 * gate then shows the state instead of a blank window). Elsewhere (`vite dev`, screenshots) the
 * seeded mock, with the scenario picked by `?mock=<scenario>` (see `MOCK_SCENARIOS`).
 */
export async function createRuntime(search: string): Promise<Runtime> {
  if (insideTauri()) {
    const dataSource = createTauriDataSource(invoke);
    const config = await dataSource.settings().then(configFromSettings, () => bootstrapConfig);
    return { dataSource, config, listen: tauriListen };
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
