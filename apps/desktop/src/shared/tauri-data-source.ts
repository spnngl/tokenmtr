import { z } from "zod";
import { type Command, ERROR_KINDS } from "./bindings";
import type { DashboardTarget, DataSource, NotificationPermissionState } from "./data-source";
import type { Filters } from "./filters";
import {
  advisorReportSchema,
  alertSchema,
  appStateSchema,
  backupSchema,
  billingReportSchema,
  branchRowSchema,
  budgetSchema,
  filterOptionsSchema,
  fxRateSchema,
  heatmapSchema,
  ingestStatusSchema,
  modelRowSchema,
  parseIssueSchema,
  projectRowSchema,
  quotaHistorySchema,
  quotaMeterSchema,
  recapSnapshotSchema,
  seriesSchema,
  sessionDetailSchema,
  sessionOriginSchema,
  sessionPageSchema,
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

/** A command failed or answered with a document that does not match its schema. */
export class DataSourceError extends Error {
  override readonly name = "DataSourceError";
}

/** A failed command as the shell serialises it (`CommandError` in `src-tauri/src/error.rs`). */
export const commandErrorSchema = z.object({ kind: z.enum(ERROR_KINDS), message: z.string() });

function describe(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  const commandError = commandErrorSchema.safeParse(error);
  if (commandError.success) {
    return commandError.data.message;
  }
  return typeof error === "string" ? error : JSON.stringify(error);
}

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
    let raw: unknown;
    try {
      raw = await invoke(command, args);
    } catch (error) {
      throw new DataSourceError(`${command}: ${describe(error)}`);
    }
    const parsed = schema.safeParse(raw);
    if (!parsed.success) {
      throw new DataSourceError(
        `${command}: unexpected response\n${z.prettifyError(parsed.error)}`,
      );
    }
    return parsed.data;
  }

  async function act(command: Command, args: Record<string, unknown> = {}): Promise<void> {
    try {
      await invoke(command, args);
    } catch (error) {
      throw new DataSourceError(`${command}: ${describe(error)}`);
    }
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
    models: (filters) => read("models", request(filters), z.array(modelRowSchema)),
    projects: (filters) => read("projects", request(filters), z.array(projectRowSchema)),
    branches: (filters) => read("branches", request(filters), z.array(branchRowSchema)),
    unresolvedCwds: (filters) =>
      read("unresolved_cwds", request(filters), z.array(unresolvedCwdSchema)),
    heatmap: (filters) => read("heatmap", request(filters), heatmapSchema),
    tools: (filters) => read("tools", request(filters), toolsReportSchema),
    signals: (filters) => read("signals", request(filters), signalsReportSchema),
    billing: (filters) => read("billing", request(filters), billingReportSchema),

    sessions: (filters, page, pageSize) =>
      read("sessions", { ...request(filters), page, pageSize }, sessionPageSchema),
    sessionDetail: (agent, session, currency) =>
      read("session_detail", { agent, session, currency }, sessionDetailSchema),
    sessionOrigin: (agent, session) =>
      read("session_origin", { agent, session }, sessionOriginSchema),

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

    scanNow: () => act("scan_now"),
    exportNow: () => act("export_now"),
    backupNow: () => act("backup_now"),
    openDashboard: (target: DashboardTarget) => act("open_dashboard", { ...target }),
    installDaemon: () => act("install_daemon"),
    restartDaemon: () => act("restart_daemon"),
    revealPath: (path) => act("reveal_path", { path }),
    addProjectRule: (cwd) => act("add_project_rule", { cwd }),
    keepTenYears: (agent, profile) => act("keep_ten_years", { agent, profile }),
    setLaunchAtLogin: (enabled) => act("set_launch_at_login", { enabled }),
    eraseAllData: () => act("erase_all_data"),
    requestNotifications: (): Promise<NotificationPermissionState> =>
      read("request_notifications", {}, notificationPermissionSchema),
    completeOnboarding: () => act("complete_onboarding"),
  };
}
