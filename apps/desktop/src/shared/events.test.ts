import { describe, expect, it, vi } from "vitest";
import { COMMANDS, ERROR_KINDS, EVENTS } from "./bindings";
import { daemonStateSchema, type Listen, navigationPath, subscribeToShell } from "./events";
import { defaultFilters } from "./filters";
import { createMockDataSource } from "./mock";
import { configFromSettings } from "./runtime";
import { commandErrorSchema, createTauriDataSource, type Invoke } from "./tauri-data-source";

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
  return { invalidateQueries: vi.fn(async () => undefined) };
}

describe("shell events", () => {
  it("refetches everything on the main window when the store changes", async () => {
    const { listen, fire } = fakeListen();
    const queryClient = fakeClient();
    await subscribeToShell({ listen, queryClient, window: "main" });
    fire(EVENTS.storeChanged, { datasets: ["usage"] });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith();
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

  it("moves the router on navigate and drops malformed targets", async () => {
    const { listen, fire } = fakeListen();
    const navigate = vi.fn();
    await subscribeToShell({ listen, queryClient: fakeClient(), window: "main", navigate });
    fire(EVENTS.navigate, { route: "/quota", agent: null });
    fire(EVENTS.navigate, { route: "/", agent: "claude" });
    fire(EVENTS.navigate, { route: "quota", agent: null });
    expect(navigate.mock.calls).toEqual([["/quota"], ["/?agent=claude"]]);
    expect(navigationPath({ route: "/", agent: "a b" })).toBe("/?agent=a%20b");
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
      daemonStateSchema.parse({ state: "incompatible", daemon_version: "9.0.0" }),
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
      tauri.sessions(filters, 0, 25),
      tauri.sessionDetail("claude", "s", "USD"),
      tauri.sessionOrigin("claude", "s"),
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
      tauri.revealPath("/tmp"),
      tauri.addProjectRule("/tmp"),
      tauri.keepTenYears("claude", "default"),
      tauri.setLaunchAtLogin(true),
      tauri.eraseAllData(),
      tauri.requestNotifications(),
      tauri.completeOnboarding(),
    ];
    await Promise.allSettled(attempts);
    const registered = new Set<string>(COMMANDS);
    for (const command of calls) {
      expect(registered.has(command), command).toBe(true);
    }
    // Commands the UI does not call through the data source yet: subscriptions and reports are
    // written by the Settings form and the Reports screen (rendered in the webview for now).
    const unused = COMMANDS.filter((command) => !calls.has(command));
    expect(unused).toEqual(["add_subscription", "generate_report", "save_report"]);
  });

  it("parses a serialised command error and knows every kind", () => {
    for (const kind of ERROR_KINDS) {
      expect(commandErrorSchema.safeParse({ kind, message: "m" }).success).toBe(true);
    }
    expect(commandErrorSchema.safeParse({ kind: "other", message: "m" }).success).toBe(false);
  });

  it("surfaces a command error's message, not its JSON", async () => {
    const tauri = createTauriDataSource(async () => {
      throw { kind: "daemon_offline", message: "the daemon is not running" };
    });
    await expect(tauri.scanNow()).rejects.toThrow("scan_now: the daemon is not running");
  });
});

describe("config from settings", () => {
  it("takes the display zone and currency and a fixed rate when configured", async () => {
    const settings = await createMockDataSource({ scenario: "populated" }).settings();
    const fixed = configFromSettings({
      ...settings,
      display: { currency: "EUR", time_zone: "Europe/Paris" },
      fx: { ...settings.fx, fixed_eur_per_usd: 0.92 },
    });
    expect(fixed.timeZone).toBe("Europe/Paris");
    expect(fixed.defaultCurrency).toBe("EUR");
    expect(fixed.fx).toEqual({ kind: "fixed", eurPerUsd: 0.92 });
    const ecb = configFromSettings({
      ...settings,
      fx: { ...settings.fx, fixed_eur_per_usd: null, offline: true },
    });
    expect(ecb.fx).toEqual({ kind: "ecb", rates: [], offline: true });
  });
});
