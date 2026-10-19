import {
  type InvalidateQueryFilters,
  type Query,
  QueryCache,
  QueryClient,
} from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { bootstrapConfig, configFromSettings } from "./app-context";
import { COMMANDS, ERROR_KINDS, EVENTS } from "./bindings";
import { commandFailure, DataSourceError } from "./errors";
import {
  daemonStateSchema,
  type Listen,
  navigationSearch,
  queriesForStoreChange,
  subscribeToShell,
} from "./events";
import { defaultFilters } from "./filters";
import { createMockDataSource } from "./mock";
import type { ReportSpec } from "./schemas";
import { createTauriDataSource, type Invoke } from "./tauri-data-source";

const REPORT_SPEC: ReportSpec = {
  range: { kind: "custom", first: "2026-06-01", last: "2026-06-30" },
  scope: { kind: "all" },
  format: "markdown",
  currency: "USD",
};

/** A `listen` whose handlers the test fires by hand. */
function fakeListen() {
  const handlers = new Map<string, (payload: unknown) => void>();
  const unsubscribed: string[] = [];
  const listen: Listen = async (event, handler) => {
    handlers.set(event, handler);
    return () => {
      unsubscribed.push(event);
    };
  };
  const fire = (event: string, payload: unknown = null) => {
    const handler = handlers.get(event);
    if (handler === undefined) {
      throw new Error(`nobody listens to ${event}`);
    }
    handler(payload);
  };
  return { listen, fire, handlers, unsubscribed };
}

function fakeClient() {
  return {
    invalidateQueries: vi.fn<(filters?: InvalidateQueryFilters) => Promise<void>>(
      async () => undefined,
    ),
  };
}

/** The part of a cached query an invalidation predicate reads. */
function fakeQuery(queryKey: readonly unknown[]): Query {
  return new QueryCache().build(new QueryClient(), { queryKey });
}

describe("shell events", () => {
  it("refetches the queries that read the changed datasets on the main window", async () => {
    const { listen, fire } = fakeListen();
    const queryClient = fakeClient();
    await subscribeToShell({ listen, queryClient, window: "main" });
    fire(EVENTS.storeChanged, { datasets: ["quota_snapshots"] });
    const filter = queryClient.invalidateQueries.mock.lastCall?.[0];
    const matches = (head: string) => filter?.predicate?.(fakeQuery([head, "x"])) === true;
    expect(matches("quota-latest")).toBe(true);
    expect(matches("app-state")).toBe(true);
    expect(matches("models")).toBe(false);
    expect(matches("report")).toBe(false);
  });

  it("maps store changes onto query keys, refreshing every store query when unsure", () => {
    const usage = queriesForStoreChange({ datasets: ["usage_events"] });
    expect(usage.has("overview")).toBe(true);
    expect(usage.has("settings")).toBe(false);
    expect(queriesForStoreChange({ datasets: ["fx_rates"] }).has("settings")).toBe(true);
    for (const unsure of [{ datasets: [] }, { datasets: ["new_dataset"] }, null]) {
      const heads = queriesForStoreChange(unsure);
      expect(heads.has("tools")).toBe(true);
      expect(heads.has("quota-history")).toBe(true);
      expect(heads.has("report")).toBe(false);
    }
  });

  it("refetches alerts and health when an alert fires", async () => {
    const { listen, fire } = fakeListen();
    const queryClient = fakeClient();
    await subscribeToShell({ listen, queryClient, window: "main" });
    fire(EVENTS.alertFired, { kind: "backup_failed" });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({ queryKey: ["alerts"] });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({ queryKey: ["system-health"] });
  });

  it("makes the recap re-query only when it is shown", async () => {
    const { listen, fire, handlers } = fakeListen();
    const queryClient = fakeClient();
    await subscribeToShell({ listen, queryClient, window: "recap" });
    expect(handlers.has(EVENTS.storeChanged)).toBe(false);
    expect(handlers.has(EVENTS.navigate)).toBe(false);
    fire(EVENTS.recapShown);
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith();
  });

  it("refetches only the app state while the daemon is away, everything once it is back", async () => {
    const { listen, fire } = fakeListen();
    const queryClient = fakeClient();
    await subscribeToShell({ listen, queryClient, window: "main" });
    fire(EVENTS.daemonState, { state: "offline" });
    expect(queryClient.invalidateQueries).toHaveBeenLastCalledWith({ queryKey: ["app-state"] });
    fire(EVENTS.daemonState, { state: "connected", daemon_version: "0.1.0" });
    expect(queryClient.invalidateQueries).toHaveBeenLastCalledWith();
  });

  it("ignores a daemon-state payload it cannot parse", async () => {
    const { listen, fire } = fakeListen();
    const queryClient = fakeClient();
    await subscribeToShell({ listen, queryClient, window: "main" });
    fire(EVENTS.daemonState, { state: "sideways" });
    expect(queryClient.invalidateQueries).not.toHaveBeenCalled();
  });

  it("moves the router on navigate and drops targets that are not screens", async () => {
    const { listen, fire } = fakeListen();
    const navigate = vi.fn();
    await subscribeToShell({ listen, queryClient: fakeClient(), window: "main", navigate });
    fire(EVENTS.navigate, { route: "/quota", agent: null });
    fire(EVENTS.navigate, { route: "/", agent: "claude" });
    fire(EVENTS.navigate, { route: "quota", agent: null });
    fire(EVENTS.navigate, { route: "/nowhere", agent: null });
    expect(navigate.mock.calls).toEqual([
      [{ route: "/quota", agent: null }],
      [{ route: "/", agent: "claude" }],
    ]);
  });

  it("keeps the current filters when navigating", () => {
    const current = { range: "7d", agent: "pi" };
    expect(navigationSearch(current, null)).toEqual(current);
    expect(navigationSearch(current, "claude")).toEqual({ range: "7d", agent: "claude" });
  });

  it("keeps and removes the listeners that registered when one fails", async () => {
    const { listen: inner, unsubscribed } = fakeListen();
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const listen: Listen = (event, handler) =>
      event === EVENTS.navigate ? Promise.reject(new Error("denied")) : inner(event, handler);
    const stop = await subscribeToShell({ listen, queryClient: fakeClient(), window: "main" });
    stop();
    expect(unsubscribed).toContain(EVENTS.storeChanged);
    expect(unsubscribed).not.toContain(EVENTS.navigate);
    expect(errors).toHaveBeenCalledOnce();
    errors.mockRestore();
  });

  it("removes every listener when unsubscribed", async () => {
    const { listen, handlers, unsubscribed } = fakeListen();
    const stop = await subscribeToShell({ listen, queryClient: fakeClient(), window: "main" });
    stop();
    expect(unsubscribed.sort()).toEqual([...handlers.keys()].sort());
  });

  it("parses the payloads the shell serialises", () => {
    expect(daemonStateSchema.parse({ state: "offline" })).toEqual({ state: "offline" });
    expect(
      daemonStateSchema.parse({ state: "incompatible", daemon_version: "9.0.0", daemon_schema: 9 }),
    ).toHaveProperty("daemon_version", "9.0.0");
  });
});

describe("command bindings", () => {
  it("lists every command the Tauri data source invokes, and nothing it does not", async () => {
    const calls = new Set<string>();
    const invoke: Invoke = async (command) => {
      calls.add(command);
      return null;
    };
    const tauri = createTauriDataSource(invoke);
    const filters = defaultFilters("USD");
    const attempts = [
      tauri.appState(),
      tauri.filterOptions(),
      tauri.settings(),
      tauri.overview(filters),
      tauri.series(filters, "agent"),
      tauri.models(filters),
      tauri.projects(filters),
      tauri.branches(filters),
      tauri.unresolvedCwds(filters),
      tauri.heatmap(filters),
      tauri.tools(filters),
      tauri.signals(filters),
      tauri.billing(filters),
      tauri.billingSeries(filters, "daily", false),
      tauri.sessions(filters, 0, 25),
      tauri.sessionDetail("claude", "s", "USD"),
      tauri.sessionOrigin("claude", "s"),
      tauri.sessionAnalytics(filters),
      tauri.sessionTimeline("claude", "s", "USD"),
      tauri.quotaLatest(filters),
      tauri.quotaHistory(filters, "claude"),
      tauri.advisor(filters),
      tauri.budgets("USD"),
      tauri.recap("week", "USD"),
      tauri.systemHealth(),
      tauri.ingestStatus(),
      tauri.parseIssues(),
      tauri.alerts(),
      tauri.backups(),
      tauri.fxRates(),
      tauri.scanNow(),
      tauri.exportNow(),
      tauri.backupNow(),
      tauri.openDashboard({ route: "/", agent: null }),
      tauri.installDaemon(),
      tauri.restartDaemon(),
      tauri.revealPath({ kind: "exports_dir" }),
      tauri.projectRuleFor("/tmp"),
      tauri.addProjectRule("/tmp", "scratch"),
      tauri.addSubscription({
        id: "claude-pro",
        provider: "anthropic",
        plan: "pro",
        agents: ["claude"],
        profiles: ["default"],
        fee: { usd: 20_000_000 },
        period: "monthly",
        start: "2026-05-01",
        end: null,
      }),
      tauri.connectKeychain(),
      tauri.keepTenYears("claude", "default"),
      tauri.setLaunchAtLogin(true),
      tauri.eraseAllData(),
      tauri.requestNotifications(),
      tauri.completeOnboarding(),
      tauri.generateReport(REPORT_SPEC),
      tauri.saveReport(REPORT_SPEC),
    ];
    await Promise.allSettled(attempts);
    const registered = new Set<string>(COMMANDS);
    for (const command of calls) {
      expect(registered.has(command), command).toBe(true);
    }
    const unused = COMMANDS.filter((command) => !calls.has(command));
    expect(unused).toEqual([]);
  });

  it("classifies a serialised command error by kind and keeps unknown kinds' text", () => {
    for (const kind of ERROR_KINDS) {
      expect(commandFailure({ kind, message: "m" })).toEqual({ kind, message: "m" });
    }
    expect(commandFailure({ kind: "other", message: "m" })).toEqual({
      kind: "unknown",
      message: "m",
    });
    expect(commandFailure("database is locked")).toEqual({
      kind: "unknown",
      message: "database is locked",
    });
  });

  it("surfaces a command error's message and kind, not its JSON", async () => {
    const tauri = createTauriDataSource(async () => {
      throw { kind: "daemon_offline", message: "the daemon is not running" };
    });
    const error = await tauri.scanNow().catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(DataSourceError);
    expect(error).toMatchObject({
      message: "the daemon is not running",
      command: "scan_now",
      failure: { kind: "daemon_offline" },
    });
  });
});

describe("config from settings", () => {
  it("takes the display zone and currency and keeps the clock", async () => {
    const settings = await createMockDataSource({ scenario: "populated" }).settings();
    const config = configFromSettings(bootstrapConfig, {
      ...settings,
      display: { currency: "EUR", time_zone: "Europe/Paris" },
    });
    expect(config).toEqual({
      ...bootstrapConfig,
      timeZone: "Europe/Paris",
      defaultCurrency: "EUR",
    });
  });
});
