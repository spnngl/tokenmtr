import { keepPreviousData, type UseQueryResult, useQuery } from "@tanstack/react-query";
import { useDataSource } from "./app-context";
import type { DataSource } from "./data-source";
import type { Filters } from "./filters";
import type { Currency } from "./format";
import type { FilterOptions, GroupBy, Period } from "./schemas";
import type { ViewState } from "./state-view";

/** One React Query hook per `DataSource` read; keys carry every input so filters refetch. */
function useRead<T>(
  key: readonly unknown[],
  read: (dataSource: DataSource) => Promise<T>,
  options: { readonly keepPrevious?: boolean; readonly refetchMs?: number } = {},
): UseQueryResult<T> {
  const dataSource = useDataSource();
  return useQuery({
    queryKey: key,
    queryFn: () => read(dataSource),
    ...(options.keepPrevious === true ? { placeholderData: keepPreviousData } : {}),
    ...(options.refetchMs === undefined ? {} : { refetchInterval: options.refetchMs }),
  });
}

export const EMPTY_FILTER_OPTIONS: FilterOptions = {
  agents: [],
  profiles: [],
  models: [],
  projects: [],
  clients: [],
};

const KEEP = { keepPrevious: true } as const;

export const useAppState = () =>
  useRead(["app-state"], (ds) => ds.appState(), { refetchMs: 30_000 });
export const useFilterOptions = () => useRead(["filter-options"], (ds) => ds.filterOptions());
export const useSettings = () => useRead(["settings"], (ds) => ds.settings());

export const useOverview = (filters: Filters) =>
  useRead(["overview", filters], (ds) => ds.overview(filters), KEEP);
export const useSeries = (filters: Filters, groupBy: GroupBy) =>
  useRead(["series", filters, groupBy], (ds) => ds.series(filters, groupBy), KEEP);
export const useModels = (filters: Filters) =>
  useRead(["models", filters], (ds) => ds.models(filters), KEEP);
export const useProjects = (filters: Filters) =>
  useRead(["projects", filters], (ds) => ds.projects(filters), KEEP);
export const useBranches = (filters: Filters) =>
  useRead(["branches", filters], (ds) => ds.branches(filters), KEEP);
export const useUnresolvedCwds = (filters: Filters) =>
  useRead(["unresolved-cwds", filters], (ds) => ds.unresolvedCwds(filters), KEEP);
export const useHeatmap = (filters: Filters) =>
  useRead(["heatmap", filters], (ds) => ds.heatmap(filters), KEEP);
export const useTools = (filters: Filters) =>
  useRead(["tools", filters], (ds) => ds.tools(filters), KEEP);
export const useSignals = (filters: Filters) =>
  useRead(["signals", filters], (ds) => ds.signals(filters), KEEP);
export const useBilling = (filters: Filters) =>
  useRead(["billing", filters], (ds) => ds.billing(filters), KEEP);

export const useSessions = (filters: Filters, page: number, pageSize: number) =>
  useRead(
    ["sessions", filters, page, pageSize],
    (ds) => ds.sessions(filters, page, pageSize),
    KEEP,
  );
export const useSessionDetail = (agent: string, session: string, currency: Currency) =>
  useRead(["session-detail", agent, session, currency], (ds) =>
    ds.sessionDetail(agent, session, currency),
  );
export const useSessionOrigin = (agent: string, session: string) =>
  useRead(["session-origin", agent, session], (ds) => ds.sessionOrigin(agent, session));

export const useQuotaLatest = (filters: Filters) =>
  useRead(["quota-latest", filters], (ds) => ds.quotaLatest(filters), KEEP);
export const useQuotaHistory = (filters: Filters, agent: string) =>
  useRead(["quota-history", filters, agent], (ds) => ds.quotaHistory(filters, agent), KEEP);
export const useAdvisor = (filters: Filters) =>
  useRead(["advisor", filters], (ds) => ds.advisor(filters), KEEP);
export const useBudgets = (currency: Currency) =>
  useRead(["budgets", currency], (ds) => ds.budgets(currency), KEEP);

export const useRecap = (period: Period, currency: Currency) =>
  useRead(["recap", period, currency], (ds) => ds.recap(period, currency), {
    keepPrevious: true,
    refetchMs: 15_000,
  });
export const useSystemHealth = () => useRead(["system-health"], (ds) => ds.systemHealth());
export const useIngestStatus = () => useRead(["ingest-status"], (ds) => ds.ingestStatus());
export const useParseIssues = () => useRead(["parse-issues"], (ds) => ds.parseIssues());
export const useAlerts = () => useRead(["alerts"], (ds) => ds.alerts());
export const useBackups = () => useRead(["backups"], (ds) => ds.backups());
export const useFxRates = () => useRead(["fx-rates"], (ds) => ds.fxRates());

export interface EmptyCopy {
  readonly title: string;
  readonly hint: string;
}

/** Empty state of every screen that aggregates usage over the filtered range. */
export const NO_USAGE_COPY: EmptyCopy = {
  title: "No usage in this range",
  hint: "Widen the range or clear a filter. If you have not run an agent yet, start a session and scan.",
};

/**
 * Maps a query onto the §7 view states: loading, error (with retry), empty (when `empty` returns
 * copy for the data) or ready.
 */
export function queryViewState<T>(
  query: UseQueryResult<T>,
  empty: (data: T) => EmptyCopy | null,
): ViewState {
  switch (query.status) {
    case "pending":
      return { kind: "loading" };
    case "error":
      return {
        kind: "error",
        message: query.error.message,
        onRetry: () => {
          void query.refetch();
        },
      };
    case "success": {
      const copy = empty(query.data);
      return copy === null ? { kind: "ready" } : { kind: "empty", ...copy };
    }
    default:
      query satisfies never;
      throw new Error("unreachable query status");
  }
}

/** Combines the states of several queries: the first non-ready state wins. */
export function firstBlockingState(states: readonly ViewState[]): ViewState {
  return states.find((state) => state.kind !== "ready") ?? { kind: "ready" };
}
