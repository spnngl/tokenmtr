import type { DataSource, NotificationPermissionState } from "../data-source";
import type { AppState, UnresolvedCwd } from "../schemas";
import { rangeOf } from "./aggregate";
import { advisorOf, quotaHistoryOf, quotaLatestOf } from "./quota";
import { recapOf } from "./recap";
import {
  billingOf,
  branchesOf,
  budgetsOf,
  factsFor,
  heatmapOf,
  modelsOf,
  projectsOf,
  seriesOf,
  signalsOf,
  toolsOf,
  totalsOf,
} from "./reports";
import { FACTS, NOW_MS } from "./seed";
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
  /** Three agents, 90 days of data, a failing sink, an open format drift. */
  "populated",
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
  /** Observes every action the UI triggers, e.g. `scanNow`. */
  readonly onAction?: (action: string, args: readonly unknown[]) => void;
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
  const tenYears = new Set<string>();
  let onboarded = scenario !== "onboarding";
  let notifications: NotificationPermissionState = "undetermined";
  let daemonInstalled = false;

  const act = (action: string, ...args: readonly unknown[]): Promise<void> => {
    options.onAction?.(action, args);
    return read(() => undefined);
  };

  function read<T>(produce: () => T): Promise<T> {
    if (scenario === "loading") {
      return new Promise<T>(() => undefined);
    }
    if (scenario === "error") {
      return Promise.reject(new Error("database is locked"));
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
        return { kind: "incompatible", app_version: "0.2.0", daemon_version: "0.1.0" };
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
    settings: () => read(() => settingsOf(tenYears)),

    overview: (filters) =>
      read(() =>
        totalsOf(
          empty ? [] : factsFor(filters),
          filters.currency,
          empty ? 0 : sessionsOf(filters, 0, 1).total,
        ),
      ),
    series: (filters, groupBy) =>
      read(() =>
        seriesOf(empty ? [] : factsFor(filters), rangeOf(filters.range), filters.currency, groupBy),
      ),
    models: (filters) => read(() => modelsOf(empty ? [] : factsFor(filters), filters.currency)),
    projects: (filters) => read(() => projectsOf(empty ? [] : factsFor(filters), filters.currency)),
    branches: (filters) => read(() => branchesOf(empty ? [] : factsFor(filters), filters.currency)),
    unresolvedCwds: () => read(() => (empty ? [] : [...UNRESOLVED])),
    heatmap: (filters) => read(() => heatmapOf(empty ? [] : factsFor(filters), filters.currency)),
    tools: (filters) => read(() => toolsOf(empty ? [] : factsFor(filters))),
    signals: (filters) => read(() => signalsOf(rangeOf(filters.range))),
    billing: (filters) => read(() => billingOf(empty ? [] : factsFor(filters), filters.currency)),

    sessions: (filters, page, pageSize) =>
      read(() =>
        empty
          ? { currency: filters.currency, total: 0, page, page_size: pageSize, rows: [] }
          : sessionsOf(filters, page, pageSize),
      ),
    sessionDetail: (agent, session, currency) =>
      read(() => sessionDetailOf(agent, session, currency)),
    sessionOrigin: (agent, session) => read(() => sessionOriginOf(agent, session)),

    quotaLatest: (filters) => read(() => (empty ? [] : quotaLatestOf(filters))),
    quotaHistory: (filters, agent) => read(() => (empty ? [] : quotaHistoryOf(filters, agent))),
    advisor: (filters) => read(() => (empty ? [] : advisorOf(filters))),
    budgets: () => read(() => (empty ? [] : budgetsOf())),

    recap: (period, currency) => read(() => recapOf(period, currency, empty)),
    systemHealth: () => read(() => systemHealthOf()),
    ingestStatus: () => read(() => ingestStatusOf()),
    parseIssues: () => read(() => [...PARSE_ISSUES]),
    alerts: () => read(() => [...ALERTS]),
    backups: () => read(() => [...BACKUPS]),
    fxRates: () => read(() => fxRatesOf()),

    scanNow: () => act("scanNow"),
    exportNow: () => act("exportNow"),
    backupNow: () => act("backupNow"),
    openDashboard: (target) => act("openDashboard", target),
    installDaemon: () => {
      daemonInstalled = true;
      return act("installDaemon");
    },
    restartDaemon: () => act("restartDaemon"),
    revealPath: (path) => act("revealPath", path),
    addProjectRule: (cwd) => act("addProjectRule", cwd),
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
    completeOnboarding: () => {
      onboarded = true;
      return act("completeOnboarding");
    },
  };
}
