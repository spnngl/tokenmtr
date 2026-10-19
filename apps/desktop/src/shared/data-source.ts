import type { Filters } from "./filters";
import type { Currency } from "./format";
import type {
  AdvisorReport,
  Alert,
  AppState,
  Backup,
  BillingReport,
  BillingSeries,
  Branches,
  Budget,
  FilterOptions,
  FxRateView,
  GeneratedReport,
  Granularity,
  GroupBy,
  Heatmap,
  IngestStatus,
  MenubarRing,
  Models,
  ParseIssue,
  Period,
  Plan,
  ProjectGrouping,
  ProjectModels,
  ProjectRemote,
  ProjectRule,
  Projects,
  QuotaHistory,
  QuotaMeter,
  RecapSnapshot,
  ReportSpec,
  Series,
  SessionAnalytics,
  SessionDetail,
  SessionOrigin,
  SessionPage,
  SessionTimeline,
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

/**
 * A subscription to add (`SubscriptionSpec` in `tokenmtr-core`): `fee` holds micro-units keyed by
 * its currency, dates are ISO days, `profiles: null` covers every profile.
 */
export interface SubscriptionSpec {
  readonly id: string;
  readonly provider: string;
  readonly plan: Plan;
  readonly agents: readonly string[];
  readonly profiles: readonly string[] | null;
  readonly fee: { readonly usd: number } | { readonly eur: number };
  readonly period: "monthly" | "yearly";
  readonly start: string;
  readonly end: string | null;
}

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
  /** Rows by project key, or by repository root (else cwd) for `"folder"`. */
  projects(filters: Filters, grouping: ProjectGrouping): Promise<Projects>;
  /** Projects split by model, biggest API value first. */
  projectModels(filters: Filters, grouping: ProjectGrouping): Promise<ProjectModels>;
  /** The git remotes of one project, by name; empty when it is not a repository. */
  projectRemotes(project: string): Promise<ProjectRemote[]>;
  /** Branches of `filters.project`. */
  branches(filters: Filters): Promise<Branches>;
  unresolvedCwds(filters: Filters): Promise<UnresolvedCwd[]>;
  heatmap(filters: Filters): Promise<Heatmap>;
  tools(filters: Filters): Promise<ToolsReport>;
  signals(filters: Filters): Promise<SignalsReport>;
  billing(filters: Filters): Promise<BillingReport>;
  /** Paid versus API value per bucket; `splitByAgent` fills each point's `agents`. */
  billingSeries(
    filters: Filters,
    granularity: Granularity,
    splitByAgent: boolean,
  ): Promise<BillingSeries>;

  // Sessions
  /** Newest activity first; `page` is 0-based. */
  sessions(filters: Filters, page: number, pageSize: number): Promise<SessionPage>;
  sessionDetail(agent: string, session: string, currency: Currency): Promise<SessionDetail>;
  sessionOrigin(agent: string, session: string): Promise<SessionOrigin>;
  /** Session charts: top 500 sessions by API value, cost histogram, starts per day and hour. */
  sessionAnalytics(filters: Filters): Promise<SessionAnalytics>;
  /** Cumulative spend, signals and tool counts of one session. */
  sessionTimeline(agent: string, session: string, currency: Currency): Promise<SessionTimeline>;

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
  /** The rule the shell suggests for `cwd`, named after its repository when known. */
  projectRuleFor(cwd: string): Promise<ProjectRule>;
  /** Adds a `[[projects]]` rule naming the sessions started in `cwd` `name`. */
  addProjectRule(cwd: string, name: string): Promise<void>;
  /** Adds a `[[subscriptions]]` entry to the config (created when missing). */
  addSubscription(spec: SubscriptionSpec): Promise<void>;
  /** Sets `[menubar] rings` and `hidden_profiles`: which quota windows get a menu-bar ring. */
  setMenubarRings(rings: readonly MenubarRing[], hiddenProfiles: readonly string[]): Promise<void>;
  /** Sets `[git] default_remote` and `default_branch` (removed when `null`). */
  setGitDefaults(defaultRemote: string, defaultBranch: string | null): Promise<void>;
  /** The daemon reads the Claude credentials once with the Keychain prompt allowed. */
  connectKeychain(): Promise<void>;
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
