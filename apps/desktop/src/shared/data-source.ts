import type { Filters } from "./filters";
import type { Currency } from "./format";
import type {
  AdvisorReport,
  Alert,
  AppState,
  Backup,
  BillingReport,
  BranchRow,
  Budget,
  FilterOptions,
  FxRateView,
  GroupBy,
  Heatmap,
  IngestStatus,
  ModelRow,
  ParseIssue,
  Period,
  ProjectRow,
  QuotaHistory,
  QuotaMeter,
  RecapSnapshot,
  Series,
  SessionDetail,
  SessionOrigin,
  SessionPage,
  Settings,
  SignalsReport,
  SystemHealth,
  ToolsReport,
  Totals,
  UnresolvedCwd,
} from "./schemas";

export type NotificationPermissionState = "granted" | "denied" | "undetermined";

export interface DashboardTarget {
  readonly route: "/" | "/settings";
  /** Pre-selects the agent filter; `null` for all agents. */
  readonly agent: string | null;
}

/**
 * Everything the UI asks of the backend. Reads resolve to schema-validated values; money is
 * already in `filters.currency` (or the `currency` argument), so the UI never converts. Each
 * method is one Tauri command of the same name in snake_case (`sessionDetail` is
 * `session_detail`); see `tauri-data-source.ts`. Actions resolve when the work is done and reject
 * with an `Error` whose message is shown to the user.
 */
export interface DataSource {
  // Shell
  appState(): Promise<AppState>;
  filterOptions(): Promise<FilterOptions>;
  settings(): Promise<Settings>;

  // Filtered aggregates
  overview(filters: Filters): Promise<Totals>;
  series(filters: Filters, groupBy: GroupBy): Promise<Series>;
  models(filters: Filters): Promise<ModelRow[]>;
  projects(filters: Filters): Promise<ProjectRow[]>;
  /** Branches of `filters.project`. */
  branches(filters: Filters): Promise<BranchRow[]>;
  unresolvedCwds(filters: Filters): Promise<UnresolvedCwd[]>;
  heatmap(filters: Filters): Promise<Heatmap>;
  tools(filters: Filters): Promise<ToolsReport>;
  signals(filters: Filters): Promise<SignalsReport>;
  billing(filters: Filters): Promise<BillingReport>;

  // Sessions
  /** Newest activity first; `page` is 0-based. */
  sessions(filters: Filters, page: number, pageSize: number): Promise<SessionPage>;
  sessionDetail(agent: string, session: string, currency: Currency): Promise<SessionDetail>;
  sessionOrigin(agent: string, session: string): Promise<SessionOrigin>;

  // Quota and budgets
  quotaLatest(filters: Filters): Promise<QuotaMeter[]>;
  quotaHistory(filters: Filters, agent: string): Promise<QuotaHistory[]>;
  advisor(filters: Filters): Promise<AdvisorReport[]>;
  budgets(currency: Currency): Promise<Budget[]>;

  // Menu-bar recap and health
  recap(period: Period, currency: Currency): Promise<RecapSnapshot>;
  systemHealth(): Promise<SystemHealth>;
  ingestStatus(): Promise<IngestStatus[]>;
  parseIssues(): Promise<ParseIssue[]>;
  alerts(): Promise<Alert[]>;
  backups(): Promise<Backup[]>;
  fxRates(): Promise<FxRateView[]>;

  // Actions
  scanNow(): Promise<void>;
  exportNow(): Promise<void>;
  backupNow(): Promise<void>;
  openDashboard(target: DashboardTarget): Promise<void>;
  installDaemon(): Promise<void>;
  restartDaemon(): Promise<void>;
  /** Shows a file or folder in the OS file manager. */
  revealPath(path: string): Promise<void>;
  /** Opens the config at a new project rule for `cwd`. */
  addProjectRule(cwd: string): Promise<void>;
  /** Sets `cleanupPeriodDays` to ten years for one profile (D53). */
  keepTenYears(agent: string, profile: string): Promise<void>;
  setLaunchAtLogin(enabled: boolean): Promise<void>;
  /** D56: the caller has already collected the typed confirmation. */
  eraseAllData(): Promise<void>;
  requestNotifications(): Promise<NotificationPermissionState>;
  completeOnboarding(): Promise<void>;
}
