import type { Filters } from "./filters";
import type { Currency } from "./format";
import type {
  AdvisorReport,
  Alert,
  AppState,
  Backup,
  BillingReport,
  Branches,
  Budget,
  FilterOptions,
  FxRateView,
  GeneratedReport,
  GroupBy,
  Heatmap,
  IngestStatus,
  Models,
  ParseIssue,
  Period,
  Projects,
  QuotaHistory,
  QuotaMeter,
  RecapSnapshot,
  ReportSpec,
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

/**
 * What `revealPath` shows in Finder (`RevealTarget` in `src-tauri/src/request.rs`): a place the
 * shell resolves itself, never a path the webview chose.
 */
export type RevealTarget =
  | { readonly kind: "config_file" }
  | { readonly kind: "exports_dir" }
  | { readonly kind: "transcript"; readonly agent: string; readonly session: string }
  | { readonly kind: "transcript_folder"; readonly agent: string; readonly session: string };

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
  models(filters: Filters): Promise<Models>;
  projects(filters: Filters): Promise<Projects>;
  /** Branches of `filters.project`. */
  branches(filters: Filters): Promise<Branches>;
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
  /** Renders a report (`tokenmtr-report`): the preview of the Reports screen. */
  generateReport(spec: ReportSpec): Promise<GeneratedReport>;

  // Actions
  scanNow(): Promise<void>;
  exportNow(): Promise<void>;
  backupNow(): Promise<void>;
  openDashboard(target: DashboardTarget): Promise<void>;
  installDaemon(): Promise<void>;
  restartDaemon(): Promise<void>;
  /** Shows `target` in the OS file manager. */
  revealPath(target: RevealTarget): Promise<void>;
  /** Opens the config at a new project rule for `cwd`. */
  addProjectRule(cwd: string): Promise<void>;
  /** Sets `cleanupPeriodDays` to ten years for one profile. */
  keepTenYears(agent: string, profile: string): Promise<void>;
  setLaunchAtLogin(enabled: boolean): Promise<void>;
  /** The caller has already collected the typed confirmation. */
  eraseAllData(): Promise<void>;
  requestNotifications(): Promise<NotificationPermissionState>;
  completeOnboarding(): Promise<void>;
  /** Renders a report and asks where to save it; resolves with the saved path, `null` on cancel. */
  saveReport(spec: ReportSpec): Promise<string | null>;
}
