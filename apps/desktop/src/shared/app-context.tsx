import { createContext, type ReactNode, useContext } from "react";
import type { DataSource } from "./data-source";
import type { Currency, Fx } from "./format";

/** Settings the UI reads from the config; Phase 11 supplies the real values. */
export interface AppConfig {
  /** IANA zone used for every day and hour bucket (D8). */
  readonly timeZone: string;
  /** `display.currency`: the filter bar's initial currency. */
  readonly defaultCurrency: Currency;
  readonly fx: Fx;
  /** Epoch milliseconds; injectable so fixtures and tests pin the clock. */
  readonly now: () => number;
}

/**
 * Config used until Phase 11 reads the daemon's: system zone (named explicitly here, once), USD,
 * and no FX rates.
 */
export const bootstrapConfig: AppConfig = {
  timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  defaultCurrency: "USD",
  fx: { kind: "ecb", rates: [], offline: false },
  now: () => Date.now(),
};

interface AppContextValue {
  readonly config: AppConfig;
  readonly dataSource: DataSource;
}

const AppContext = createContext<AppContextValue | null>(null);

export function AppProvider(props: AppContextValue & { readonly children: ReactNode }): ReactNode {
  const { children, ...value } = props;
  return <AppContext value={value}>{children}</AppContext>;
}

function useAppContext(): AppContextValue {
  const value = useContext(AppContext);
  if (value === null) {
    throw new Error("useAppConfig and useDataSource must be used inside AppProvider");
  }
  return value;
}

/** @throws {Error} outside an `AppProvider`. */
export function useAppConfig(): AppConfig {
  return useAppContext().config;
}

/** @throws {Error} outside an `AppProvider`. */
export function useDataSource(): DataSource {
  return useAppContext().dataSource;
}
