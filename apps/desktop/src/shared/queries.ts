import { keepPreviousData, type UseQueryResult, useQuery } from "@tanstack/react-query";
import { useDataSource } from "./app-context";
import type { DataSource } from "./data-source";
import type { Filters } from "./filters";
import type { Currency } from "./format";
import type { FilterOptions, Granularity, GroupBy, Os, Period, ProjectGrouping } from "./schemas";

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
/**
 * The OS the app runs on, for the copy that has no data signal. Screens render only once the app
 * state is `ready`, which carries it; macOS stands in while it loads.
 */
export function useOs(): Os {
  const { data } = useAppState();
  return data?.kind === "ready" ? data.os : "macos";
}
export const useFilterOptions = () => useRead(["filter-options"], (ds) => ds.filterOptions());
export const useSettings = () => useRead(["settings"], (ds) => ds.settings());

export const useOverview = (filters: Filters) =>
  useRead(["overview", filters], (ds) => ds.overview(filters), KEEP);
export const useSeries = (filters: Filters, groupBy: GroupBy) =>
  useRead(["series", filters, groupBy], (ds) => ds.series(filters, groupBy), KEEP);
export const useModels = (filters: Filters) =>
  useRead(["models", filters], (ds) => ds.models(filters), KEEP);
export const useProjects = (filters: Filters, grouping: ProjectGrouping = "project") =>
  useRead(["projects", filters, grouping], (ds) => ds.projects(filters, grouping), KEEP);
export const useProjectModels = (filters: Filters, grouping: ProjectGrouping) =>
  useRead(["project-models", filters, grouping], (ds) => ds.projectModels(filters, grouping), KEEP);
export const useProjectRemotes = (project: string) =>
  useRead(["project-remotes", project], (ds) => ds.projectRemotes(project));
export const useBranches = (filters: Filters) =>
  useRead(["branches", filters], (ds) => ds.branches(filters), KEEP);
export const useUnresolvedCwds = (filters: Filters) =>
  useRead(["unresolved-cwds", filters], (ds) => ds.unresolvedCwds(filters), KEEP);
export const useHeatmap = (filters: Filters) =>
  useRead(["heatmap", filters], (ds) => ds.heatmap(filters), KEEP);
/** Keyed without the range and currency, which the calendar ignores, so changing them reuses it. */
export function useActivityCalendar(filters: Filters) {
  const { range: _range, currency: _currency, ...scope } = filters;
  return useRead(["activity-calendar", scope], (ds) => ds.activityCalendar(filters), KEEP);
}
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
export const useSessionAnalytics = (filters: Filters) =>
  useRead(["session-analytics", filters], (ds) => ds.sessionAnalytics(filters), KEEP);
export const useSessionTimeline = (agent: string, session: string, currency: Currency) =>
  useRead(["session-timeline", agent, session, currency], (ds) =>
    ds.sessionTimeline(agent, session, currency),
  );
export const useBillingSeries = (
  filters: Filters,
  granularity: Granularity,
  splitByAgent: boolean,
) =>
  useRead(
    ["billing-series", filters, granularity, splitByAgent],
    (ds) => ds.billingSeries(filters, granularity, splitByAgent),
    KEEP,
  );

export const useQuotaLatest = (filters: Filters) =>
  useRead(["quota-latest", filters], (ds) => ds.quotaLatest(filters), KEEP);
export const useQuotaHistory = (filters: Filters, agent: string) =>
  useRead(["quota-history", filters, agent], (ds) => ds.quotaHistory(filters, agent), KEEP);
export const useAdvisor = (filters: Filters) =>
  useRead(["advisor", filters], (ds) => ds.advisor(filters), KEEP);
export const useBudgets = (currency: Currency) =>
  useRead(["budgets", currency], (ds) => ds.budgets(currency), KEEP);

export const useRecap = (period: Period, currency: Currency) =>
  useRead(["recap", period, currency], (ds) => ds.recap(period, currency), KEEP);
export const useSystemHealth = () => useRead(["system-health"], (ds) => ds.systemHealth());
export const useParseIssues = () => useRead(["parse-issues"], (ds) => ds.parseIssues());
export const useAlerts = () => useRead(["alerts"], (ds) => ds.alerts());

/** The project rule the app suggests for an unresolved directory (name from git or Superset). */
export const useProjectRuleFor = (cwd: string) =>
  useRead(["project-rule-for", cwd], (ds) => ds.projectRuleFor(cwd));
