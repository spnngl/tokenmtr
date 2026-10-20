import { z } from "zod";
import type { Command } from "./bindings";
import type { DashboardTarget, DataSource, NotificationPermissionState } from "./data-source";
import { commandFailure, DataSourceError } from "./errors";
import type { Filters } from "./filters";
import {
  activityCalendarSchema,
  advisorReportSchema,
  alertSchema,
  appStateSchema,
  backupSchema,
  billingReportSchema,
  billingSeriesSchema,
  branchesSchema,
  budgetSchema,
  filterOptionsSchema,
  fxRateSchema,
  generatedReportSchema,
  heatmapSchema,
  ingestStatusSchema,
  modelsSchema,
  parseIssueSchema,
  projectModelsSchema,
  projectRemotesSchema,
  projectRuleSchema,
  projectsSchema,
  quotaHistorySchema,
  quotaMeterSchema,
  recapSnapshotSchema,
  seriesSchema,
  sessionAnalyticsSchema,
  sessionDetailSchema,
  sessionOriginSchema,
  sessionPageSchema,
  sessionTimelineSchema,
  settingsSchema,
  signalsReportSchema,
  systemHealthSchema,
  toolsReportSchema,
  toQueryRequest,
  totalsSchema,
  unresolvedCwdSchema,
} from "./schemas";

/** The transport: Tauri's `invoke` in the app, a stub in tests. */
export type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

const notificationPermissionSchema = z.enum(["granted", "denied", "undetermined"]);

/**
 * The `DataSource` over Tauri commands. Command names are the method names in snake_case. Filtered
 * queries take `{ request: QueryRequest }` (`toQueryRequest(filters)`); argument keys are
 * camelCase, which Tauri maps to the Rust parameter names. Every answer is parsed with its Zod
 * schema, so a Rust/TypeScript drift fails loudly here instead of rendering `undefined`.
 */
export function createTauriDataSource(invoke: Invoke): DataSource {
  async function read<S extends z.ZodType>(
    command: Command,
    args: Record<string, unknown>,
    schema: S,
  ): Promise<z.output<S>> {
    const raw = await call(command, args);
    const parsed = schema.safeParse(raw);
    if (!parsed.success) {
      const detail = z.prettifyError(parsed.error);
      // The user sees a sentence; the field-level mismatch goes to the console for bug reports.
      console.error(`${command}: unexpected response\n${detail}`);
      throw new DataSourceError(command, { kind: "schema", detail });
    }
    return parsed.data;
  }

  async function call(command: Command, args: Record<string, unknown> = {}): Promise<unknown> {
    try {
      return await invoke(command, args);
    } catch (error) {
      throw new DataSourceError(command, commandFailure(error));
    }
  }

  async function act(command: Command, args: Record<string, unknown> = {}): Promise<void> {
    await call(command, args);
  }

  const request = (filters: Filters): Record<string, unknown> => ({
    request: toQueryRequest(filters),
  });

  return {
    appState: () => read("app_state", {}, appStateSchema),
    filterOptions: () => read("filter_options", {}, filterOptionsSchema),
    settings: () => read("settings", {}, settingsSchema),

    overview: (filters) => read("overview", request(filters), totalsSchema),
    series: (filters, groupBy) => read("series", { ...request(filters), groupBy }, seriesSchema),
    models: (filters) => read("models", request(filters), modelsSchema),
    projects: (filters, grouping) =>
      read("projects", { ...request(filters), grouping }, projectsSchema),
    projectModels: (filters, grouping) =>
      read("project_models", { ...request(filters), grouping }, projectModelsSchema),
    projectRemotes: (project) => read("project_remotes", { project }, projectRemotesSchema),
    branches: (filters) => read("branches", request(filters), branchesSchema),
    unresolvedCwds: (filters) =>
      read("unresolved_cwds", request(filters), z.array(unresolvedCwdSchema)),
    heatmap: (filters) => read("heatmap", request(filters), heatmapSchema),
    activityCalendar: (filters) =>
      read("activity_calendar", request(filters), activityCalendarSchema),
    tools: (filters) => read("tools", request(filters), toolsReportSchema),
    signals: (filters) => read("signals", request(filters), signalsReportSchema),
    billing: (filters) => read("billing", request(filters), billingReportSchema),
    billingSeries: (filters, granularity, splitByAgent) =>
      read(
        "billing_series",
        { ...request(filters), granularity, splitByAgent },
        billingSeriesSchema,
      ),

    sessions: (filters, page, pageSize) =>
      read("sessions", { ...request(filters), page, pageSize }, sessionPageSchema),
    sessionDetail: (agent, session, currency) =>
      read("session_detail", { agent, session, currency }, sessionDetailSchema),
    sessionOrigin: (agent, session) =>
      read("session_origin", { agent, session }, sessionOriginSchema),
    sessionAnalytics: (filters) =>
      read("session_analytics", request(filters), sessionAnalyticsSchema),
    sessionTimeline: (agent, session, currency) =>
      read("session_timeline", { agent, session, currency }, sessionTimelineSchema),

    quotaLatest: (filters) => read("quota_latest", request(filters), z.array(quotaMeterSchema)),
    quotaHistory: (filters, agent) =>
      read("quota_history", { ...request(filters), agent }, z.array(quotaHistorySchema)),
    advisor: (filters) => read("advisor", request(filters), z.array(advisorReportSchema)),
    budgets: (currency) => read("budgets", { currency }, z.array(budgetSchema)),

    recap: (period, currency) => read("recap", { period, currency }, recapSnapshotSchema),
    systemHealth: () => read("system_health", {}, systemHealthSchema),
    ingestStatus: () => read("ingest_status", {}, z.array(ingestStatusSchema)),
    parseIssues: () => read("parse_issues", {}, z.array(parseIssueSchema)),
    alerts: () => read("alerts", {}, z.array(alertSchema)),
    backups: () => read("backups", {}, z.array(backupSchema)),
    fxRates: () => read("fx_rates", {}, z.array(fxRateSchema)),
    generateReport: (spec) => read("generate_report", { request: spec }, generatedReportSchema),

    scanNow: () => act("scan_now"),
    exportNow: () => act("export_now"),
    backupNow: () => act("backup_now"),
    openDashboard: (target: DashboardTarget) => act("open_dashboard", { ...target }),
    fitRecap: (height) => act("fit_recap", { height }),
    installDaemon: () => act("install_daemon"),
    restartDaemon: () => act("restart_daemon"),
    revealPath: (target) => act("reveal_path", { target }),
    editConfig: () => act("edit_config"),
    quit: () => act("quit"),
    projectRuleFor: (cwd) => read("project_rule_for", { cwd }, projectRuleSchema),
    addProjectRule: (cwd, name) => act("add_project_rule", { cwd, name }),
    addSubscription: (spec) => act("add_subscription", { spec }),
    // Tauri maps the camelCase key onto the command's `hidden_profiles` argument.
    setMenubarRings: (rings, hiddenProfiles) => act("set_menubar_rings", { rings, hiddenProfiles }),
    setGitDefaults: (defaultRemote, defaultBranch) =>
      act("set_git_defaults", { defaultRemote, defaultBranch }),
    connectKeychain: () => act("connect_keychain"),
    keepTenYears: (agent, profile) => act("keep_ten_years", { agent, profile }),
    setLaunchAtLogin: (enabled) => act("set_launch_at_login", { enabled }),
    eraseAllData: () => act("erase_all_data"),
    requestNotifications: (): Promise<NotificationPermissionState> =>
      read("request_notifications", {}, notificationPermissionSchema),
    completeOnboarding: () => act("complete_onboarding"),
    saveReport: (spec) => read("save_report", { request: spec }, z.string().nullable()),
  };
}
