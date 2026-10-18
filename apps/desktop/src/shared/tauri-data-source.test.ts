import { describe, expect, it, vi } from "vitest";
import type { DataSource } from "./data-source";
import { defaultFilters } from "./filters";
import { createMockDataSource } from "./mock";
import { toQueryRequest } from "./schemas";
import { createTauriDataSource, DataSourceError, type Invoke } from "./tauri-data-source";

const FILTERS = { ...defaultFilters("USD"), range: { kind: "preset", preset: "30d" } } as const;

/**
 * An `invoke` that answers each command with the mock data source's result. The adapter then
 * parses it with the Zod schemas, so this proves every mock fixture matches the wire schemas.
 */
function mockInvoke(mock: DataSource): Invoke {
  const answers: Record<string, () => Promise<unknown>> = {
    app_state: () => mock.appState(),
    filter_options: () => mock.filterOptions(),
    settings: () => mock.settings(),
    overview: () => mock.overview(FILTERS),
    series: () => mock.series(FILTERS, "agent"),
    models: () => mock.models(FILTERS),
    projects: () => mock.projects(FILTERS),
    branches: () => mock.branches({ ...FILTERS, project: "tokenmtr" }),
    unresolved_cwds: () => mock.unresolvedCwds(FILTERS),
    heatmap: () => mock.heatmap(FILTERS),
    tools: () => mock.tools(FILTERS),
    signals: () => mock.signals(FILTERS),
    billing: () => mock.billing(FILTERS),
    sessions: () => mock.sessions(FILTERS, 0, 25),
    session_detail: async () => {
      const page = await mock.sessions(FILTERS, 0, 1);
      const row = page.rows[0];
      if (row === undefined) {
        throw new Error("no mock session");
      }
      return mock.sessionDetail(row.agent, row.session, "USD");
    },
    session_origin: () => mock.sessionOrigin("pi", "any"),
    quota_latest: () => mock.quotaLatest(FILTERS),
    quota_history: () => mock.quotaHistory(FILTERS, "claude"),
    advisor: () => mock.advisor(FILTERS),
    budgets: () => mock.budgets("USD"),
    recap: () => mock.recap("week", "USD"),
    system_health: () => mock.systemHealth(),
    ingest_status: () => mock.ingestStatus(),
    parse_issues: () => mock.parseIssues(),
    alerts: () => mock.alerts(),
    backups: () => mock.backups(),
    fx_rates: () => mock.fxRates(),
    request_notifications: () => mock.requestNotifications(),
  };
  return (command) => {
    const answer = answers[command];
    return answer === undefined ? Promise.resolve(null) : answer();
  };
}

describe("Tauri data source over the mock fixtures", () => {
  it.each([
    "populated",
    "empty",
    "partial",
    "stale",
    "offline",
    "incompatible",
    "onboarding",
  ] as const)("parses every read of the %s scenario", async (scenario) => {
    const tauri = createTauriDataSource(mockInvoke(createMockDataSource({ scenario })));
    const first = await tauri.sessions(FILTERS, 0, 1);
    await Promise.all([
      tauri.appState(),
      tauri.filterOptions(),
      tauri.settings(),
      tauri.overview(FILTERS),
      tauri.series(FILTERS, "agent"),
      tauri.models(FILTERS),
      tauri.projects(FILTERS),
      tauri.branches(FILTERS),
      tauri.unresolvedCwds(FILTERS),
      tauri.heatmap(FILTERS),
      tauri.tools(FILTERS),
      tauri.signals(FILTERS),
      tauri.billing(FILTERS),
      tauri.sessionOrigin("pi", "any"),
      tauri.quotaLatest(FILTERS),
      tauri.quotaHistory(FILTERS, "claude"),
      tauri.advisor(FILTERS),
      tauri.budgets("USD"),
      tauri.recap("week", "USD"),
      tauri.systemHealth(),
      tauri.ingestStatus(),
      tauri.parseIssues(),
      tauri.alerts(),
      tauri.backups(),
      tauri.fxRates(),
      tauri.requestNotifications(),
    ]);
    if (scenario === "populated") {
      expect(first.rows.length).toBeGreaterThan(0);
      await tauri.sessionDetail("claude", "x", "USD");
    }
  });

  it("rejects a document that does not match the schema", async () => {
    const tauri = createTauriDataSource(async () => ({ currency: "USD" }));
    await expect(tauri.overview(FILTERS)).rejects.toThrow(/overview: unexpected response/);
  });

  it("wraps a failing command", async () => {
    const tauri = createTauriDataSource(async () => {
      throw "database is locked";
    });
    const error = await tauri.scanNow().catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(DataSourceError);
    expect(error).toHaveProperty("message", "scan_now: database is locked");
  });

  it("sends the filters as a query request", async () => {
    const invoke = vi.fn<Invoke>(async () => []);
    const filters = { ...FILTERS, agent: "pi", currency: "EUR", billing: "api" } as const;
    await createTauriDataSource(invoke).models(filters);
    expect(invoke).toHaveBeenCalledWith("models", { request: toQueryRequest(filters) });
    expect(toQueryRequest(filters)).toEqual({
      currency: "EUR",
      filters: {
        range: { kind: "last_days", days: 30 },
        agents: ["pi"],
        profiles: [],
        models: [],
        projects: [],
        clients: [],
        billing: "api",
      },
    });
  });

  it("makes a custom range's end exclusive", () => {
    const request = toQueryRequest({
      ...FILTERS,
      range: { kind: "custom", from: "2026-02-27", to: "2026-02-28" },
    });
    expect(request.filters.range).toEqual({
      kind: "custom",
      start: "2026-02-27",
      end: "2026-03-01",
    });
  });
});
