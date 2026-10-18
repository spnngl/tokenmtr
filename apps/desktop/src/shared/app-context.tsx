import { createContext, useContext } from "react";
import type { DataSource } from "./data-source";
import type { Currency } from "./format";
import type { Settings } from "./schemas";

/** What the UI reads from the config, plus the clock. */
export interface AppConfig {
  /** IANA zone used for every day and hour bucket (D8). */
  readonly timeZone: string;
  /** `display.currency`: the filter bar's initial currency. */
  readonly defaultCurrency: Currency;
  /** Epoch milliseconds; injectable so fixtures and tests pin the clock. */
  readonly now: () => number;
}

/**
 * Config used until the shell's settings arrive (or when it cannot answer): system zone (named
 * explicitly here, once) and USD.
 */
export const bootstrapConfig: AppConfig = {
  timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  defaultCurrency: "USD",
  now: () => Date.now(),
};

/** `base` with the display zone and currency the shell's settings report. */
export function configFromSettings(base: AppConfig, settings: Settings): AppConfig {
  return {
    ...base,
    timeZone: settings.display.time_zone,
    defaultCurrency: settings.display.currency,
  };
}

/** Filled by `AppProvider` (`app-provider.tsx`); read through the hooks below. */
export const DataSourceContext = createContext<DataSource | null>(null);
export const AppConfigContext = createContext<AppConfig | null>(null);

/** @throws {Error} outside an `AppProvider`. */
export function useAppConfig(): AppConfig {
  const config = useContext(AppConfigContext);
  if (config === null) {
    throw new Error("useAppConfig must be used inside AppProvider");
  }
  return config;
}

/** @throws {Error} outside an `AppProvider`. */
export function useDataSource(): DataSource {
  const dataSource = useContext(DataSourceContext);
  if (dataSource === null) {
    throw new Error("useDataSource must be used inside AppProvider");
  }
  return dataSource;
}
