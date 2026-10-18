import type { Period, RecapModel } from "./recap-model";

/** Values offered by the filter bar's selects. */
export interface FilterOptions {
  readonly agents: readonly string[];
  readonly profiles: readonly string[];
  readonly models: readonly string[];
  readonly projects: readonly string[];
  readonly clients: readonly string[];
}

/** Where `openDashboard` lands in the full app (D29 deep link). */
export interface DashboardTarget {
  readonly route: "/" | "/settings";
  readonly agent: string | null;
}

/**
 * Everything the UI needs from the backend. Phase 11 implements it with Tauri commands; until
 * then `emptyDataSource` answers as a machine with no agents.
 */
export interface DataSource {
  filterOptions(): Promise<FilterOptions>;
  recap(period: Period): Promise<RecapModel>;
  openDashboard(target: DashboardTarget): Promise<void>;
  scanNow(): Promise<void>;
}

export const EMPTY_FILTER_OPTIONS: FilterOptions = {
  agents: [],
  profiles: [],
  models: [],
  projects: [],
  clients: [],
};

export const emptyDataSource: DataSource = {
  filterOptions: () => Promise.resolve(EMPTY_FILTER_OPTIONS),
  recap: (period) =>
    Promise.resolve({
      period,
      nowMs: Date.now(),
      asOf: new Date().toISOString().slice(0, 10),
      health: "ok",
      tabs: [],
    }),
  openDashboard: () => Promise.resolve(),
  scanNow: () => Promise.resolve(),
};
