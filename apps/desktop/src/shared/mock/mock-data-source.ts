import type { DataSource, NotificationPermissionState } from "../data-source";
import { DataSourceError } from "../errors";
import type { Filters } from "../filters";
import type { Currency } from "../format";
import type { AppState, Settings, UnresolvedCwd } from "../schemas";
import { rangeOf } from "./aggregate";
import { billingSeriesOf, sessionAnalyticsOf, sessionTimelineOf } from "./analytics";
import { advisorOf, quotaHistoryOf, quotaLatestOf } from "./quota";
import { recapOf } from "./recap";
import { generateMockReport } from "./report";
import {
  billingOf,
  branchesOf,
  budgetsOf,
  extraFactsFor,
  factsFor,
  heatmapOf,
  modelsOf,
  projectModelsOf,
  projectRemotesOf,
  projectsOf,
  seriesOf,
  signalsOf,
  toolsOf,
  totalsOf,
} from "./reports";
import { FACTS, NOW_MS, SESSIONS } from "./seed";
import { sessionDetailOf, sessionOriginOf, sessionsOf } from "./sessions";
import {
  ALERTS,
  agentInfos,
  BACKUPS,
  filterOptionsOf,
  fxRatesOf,
  ingestStatusOf,
  PARSE_ISSUES,
  settingsOf,
  systemHealthOf,
} from "./system";

export const MOCK_SCENARIOS = [
  /** Three agents, 90 days of data, Claude extra usage, a failing sink, an open format drift. */
  "populated",
  /** Populated, with Claude extra usage switched off (no meter, no billing line). */
  "no-extra-usage",
  /** Populated, with one collector failing. */
  "partial",
  /** Populated, last scan older than twice the scan interval. */
  "stale",
  /** Agents followed, no usage yet. */
  "empty",
  "offline",
  "incompatible",
  "onboarding",
  /** Every read rejects. */
  "error",
  /** Every read stays pending. */
  "loading",
] as const;
export type MockScenario = (typeof MOCK_SCENARIOS)[number];

export function isMockScenario(value: string | null): value is MockScenario {
  return MOCK_SCENARIOS.some((scenario) => scenario === value);
}

export interface MockOptions {
  readonly scenario?: MockScenario;
  /** Simulated response time; defaults to none so tests stay synchronous-ish. */
  readonly latencyMs?: number;
  /** Observes every action the UI triggers, e.g. `scanNow`; throwing makes the action fail. */
  readonly onAction?: (action: string, args: readonly unknown[]) => void;
  /**
   * No FX rate is known: like the backend, a EUR request is answered in USD with
   * `currency_fallback` set.
   */
  readonly noFxRates?: boolean;
}

const SCAN_INTERVAL_SECS = 60;
const UNRESOLVED: readonly UnresolvedCwd[] = [
  {
    cwd: "/Users/dev/scratch/experiments",
    events: 420,
    api_value_usd_micros: 18_400_000,
    last_at: "2026-06-14T17:20:00Z",
  },
  {
    cwd: "/tmp/repro-1432",
    events: 64,
    api_value_usd_micros: 2_100_000,
    last_at: "2026-06-12T09:05:00Z",
  },
];

function isoAgo(ms: number): string {
  return new Date(NOW_MS - ms).toISOString().replace(".000Z", "Z");
}

/** A deterministic in-memory `DataSource`: tests, screenshots and `vite dev` outside Tauri. */
export function createMockDataSource(options: MockOptions = {}): DataSource {
  const scenario = options.scenario ?? "populated";
  const latencyMs = options.latencyMs ?? 0;
  const empty = scenario === "empty";
  const extraUsage = !empty && scenario !== "no-extra-usage";
  /** Claude extra-usage rows under `filters`, when the scenario has any. */
  const extrasFor = (filters: Filters) => (extraUsage ? extraFactsFor(filters) : []);
  const tenYears = new Set<string>();
  let menubarRings: Pick<Settings["tray"], "rings" | "hidden_profiles"> = {
    rings: ["session", "weekly"],
    hidden_profiles: [],
  };
  let git: Settings["git"] = { default_remote: "origin", default_branch: null };
  let onboarded = scenario !== "onboarding";
  let notifications: NotificationPermissionState = "undetermined";
  let daemonInstalled = false;
  /** The currency the backend answers in when asked for `requested`. */
  const answered = (requested: Currency): Currency =>
    options.noFxRates === true ? "USD" : requested;

  const act = async (action: string, ...args: readonly unknown[]): Promise<void> => {
    options.onAction?.(action, args);
    return read(() => undefined);
  };

  function read<T>(produce: () => T): Promise<T> {
    if (scenario === "loading") {
      return new Promise<T>(() => undefined);
    }
    if (scenario === "error") {
      // The shape the Tauri data source gives a failed command (a `store` `CommandError`).
      return Promise.reject(
        new DataSourceError("mock", { kind: "store", message: "database is locked" }),
      );
    }
    return new Promise<T>((resolve, reject) => {
      const run = () => {
        try {
          resolve(produce());
        } catch (error) {
          reject(error instanceof Error ? error : new Error("mock failure"));
        }
      };
      if (latencyMs > 0) {
        setTimeout(run, latencyMs);
      } else {
        queueMicrotask(run);
      }
    });
  }

  function appState(): AppState {
    switch (scenario) {
      case "offline":
        return { kind: "daemon_offline" };
      case "incompatible":
        return {
          kind: "incompatible",
          app_version: "0.2.0",
          daemon_version: "0.1.0",
          app_schema: 5,
          daemon_schema: 4,
        };
      case "onboarding":
        if (!onboarded) {
          return {
            kind: "onboarding",
            agents: agentInfos(tenYears),
            daemon_installed: daemonInstalled,
            notifications,
          };
        }
        return {
          kind: "ready",
          last_scan_at: isoAgo(30_000),
          scan_interval_secs: SCAN_INTERVAL_SECS,
          failed_collectors: 0,
        };
      case "stale":
        return {
          kind: "ready",
          last_scan_at: isoAgo(SCAN_INTERVAL_SECS * 5 * 1000),
          scan_interval_secs: SCAN_INTERVAL_SECS,
          failed_collectors: 0,
        };
      case "partial":
        return {
          kind: "ready",
          last_scan_at: isoAgo(30_000),
          scan_interval_secs: SCAN_INTERVAL_SECS,
          failed_collectors: 1,
        };
      case "populated":
      case "no-extra-usage":
      case "empty":
      case "error":
      case "loading":
        return {
          kind: "ready",
          last_scan_at: isoAgo(30_000),
          scan_interval_secs: SCAN_INTERVAL_SECS,
          failed_collectors: 0,
        };
      default:
        scenario satisfies never;
        throw new Error("unreachable scenario");
    }
  }

  return {
    // `appState` ignores the error/loading scenarios: those exercise the screens, not the gate.
    appState: () => Promise.resolve(appState()),
    filterOptions: () => read(() => filterOptionsOf(empty ? [] : FACTS)),
    settings: () =>
      read(() => {
        const settings = settingsOf(tenYears);
        return { ...settings, tray: { ...settings.tray, ...menubarRings }, git };
      }),

    overview: (filters) =>
      read(() => ({
        ...totalsOf(
          empty ? [] : factsFor(filters),
          answered(filters.currency),
          empty ? 0 : sessionsOf(filters, 0, 1).total,
          extrasFor(filters),
        ),
        currency_fallback: answered(filters.currency) !== filters.currency,
      })),
    series: (filters, groupBy) =>
      read(() =>
        seriesOf(
          empty ? [] : factsFor(filters),
          rangeOf(filters.range),
          answered(filters.currency),
          groupBy,
          extrasFor(filters),
        ),
      ),
    models: (filters) => {
      const currency = answered(filters.currency);
      return read(() => ({ currency, rows: modelsOf(empty ? [] : factsFor(filters), currency) }));
    },
    projects: (filters, grouping) => {
      const currency = answered(filters.currency);
      return read(() => ({
        currency,
        rows: projectsOf(empty ? [] : factsFor(filters), currency, grouping),
      }));
    },
    projectModels: (filters, grouping) => {
      const currency = answered(filters.currency);
      return read(() => ({
        currency,
        rows: projectModelsOf(empty ? [] : factsFor(filters), currency, grouping),
      }));
    },
    projectRemotes: (project) => read(() => (empty ? [] : projectRemotesOf(project))),
    branches: (filters) => {
      const currency = answered(filters.currency);
      return read(() => ({
        currency,
        rows: branchesOf(empty ? [] : factsFor(filters), currency),
      }));
    },
    unresolvedCwds: () => read(() => (empty ? [] : [...UNRESOLVED])),
    heatmap: (filters) =>
      read(() => heatmapOf(empty ? [] : factsFor(filters), answered(filters.currency))),
    tools: (filters) => read(() => toolsOf(empty ? [] : factsFor(filters))),
    signals: (filters) => read(() => signalsOf(rangeOf(filters.range))),
    billing: (filters) =>
      read(() =>
        billingOf(empty ? [] : factsFor(filters), answered(filters.currency), extrasFor(filters)),
      ),
    billingSeries: (filters, granularity, splitByAgent) =>
      read(() =>
        billingSeriesOf(
          empty ? [] : factsFor(filters),
          extrasFor(filters),
          rangeOf(filters.range),
          answered(filters.currency),
          granularity,
          splitByAgent,
        ),
      ),

    sessions: (filters, page, pageSize) => {
      const answeredFilters = { ...filters, currency: answered(filters.currency) };
      return read(() =>
        empty
          ? { currency: answeredFilters.currency, total: 0, page, page_size: pageSize, rows: [] }
          : sessionsOf(answeredFilters, page, pageSize),
      );
    },
    sessionDetail: (agent, session, currency) =>
      read(() => sessionDetailOf(agent, session, answered(currency))),
    sessionOrigin: (agent, session) => read(() => sessionOriginOf(agent, session)),
    sessionAnalytics: (filters) =>
      read(() => sessionAnalyticsOf(empty ? [] : SESSIONS, filters, answered(filters.currency))),
    sessionTimeline: (agent, session, currency) =>
      read(() => sessionTimelineOf(agent, session, answered(currency))),

    quotaLatest: (filters) =>
      read(() =>
        empty
          ? []
          : quotaLatestOf({ ...filters, currency: answered(filters.currency) }, extraUsage),
      ),
    quotaHistory: (filters, agent) => read(() => (empty ? [] : quotaHistoryOf(filters, agent))),
    advisor: (filters) => read(() => (empty ? [] : advisorOf(filters))),
    budgets: () => read(() => (empty ? [] : budgetsOf(options.noFxRates === true))),

    recap: (period, currency) =>
      read(() => ({
        ...recapOf(period, answered(currency), empty, extraUsage),
        currency_fallback: answered(currency) !== currency,
      })),
    systemHealth: () => read(() => systemHealthOf()),
    ingestStatus: () => read(() => ingestStatusOf()),
    parseIssues: () => read(() => [...PARSE_ISSUES]),
    alerts: () => read(() => [...ALERTS]),
    backups: () => read(() => [...BACKUPS]),
    fxRates: () => read(() => fxRatesOf()),
    generateReport: (spec) => read(() => generateMockReport(spec, empty)),

    scanNow: () => act("scanNow"),
    exportNow: () => act("exportNow"),
    backupNow: () => act("backupNow"),
    openDashboard: (target) => act("openDashboard", target),
    // The browser has no popover window to resize.
    fitRecap: () => Promise.resolve(),
    installDaemon: () => {
      daemonInstalled = true;
      return act("installDaemon");
    },
    restartDaemon: () => act("restartDaemon"),
    revealPath: (target) => act("revealPath", target),
    projectRuleFor: (cwd) => read(() => ({ match: cwd, name: cwd.split("/").pop() ?? "project" })),
    addProjectRule: (cwd, name) => act("addProjectRule", cwd, name),
    addSubscription: (spec) => act("addSubscription", spec),
    setMenubarRings: (rings, hiddenProfiles) => {
      menubarRings = { rings: [...rings], hidden_profiles: [...hiddenProfiles] };
      return act("setMenubarRings", rings, hiddenProfiles);
    },
    setGitDefaults: (defaultRemote, defaultBranch) => {
      git = { default_remote: defaultRemote, default_branch: defaultBranch };
      return act("setGitDefaults", defaultRemote, defaultBranch);
    },
    connectKeychain: () => act("connectKeychain"),
    keepTenYears: (agent, profile) => {
      tenYears.add(`${agent}:${profile}`);
      return act("keepTenYears", agent, profile);
    },
    setLaunchAtLogin: (enabled) => act("setLaunchAtLogin", enabled),
    eraseAllData: () => act("eraseAllData"),
    requestNotifications: () => {
      notifications = "granted";
      options.onAction?.("requestNotifications", []);
      return read(() => notifications);
    },
    saveReport: (spec) => {
      options.onAction?.("saveReport", [spec]);
      return read(() => `/Users/demo/Documents/${generateMockReport(spec, empty).filename}`);
    },
    completeOnboarding: () => {
      onboarded = true;
      return act("completeOnboarding");
    },
  };
}
