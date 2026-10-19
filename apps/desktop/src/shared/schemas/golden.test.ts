/**
 * Golden responses of the read commands, parsed through the real Tauri data source.
 *
 * Each `__fixtures__/<command>.json` is serialized from the real Rust view types by
 * `apps/desktop/src-tauri/src/goldens.rs` (regenerate with `just goldens`), so a Zod schema that
 * drifts from Rust fails here, and a Rust shape change fails the Rust golden test.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { DataSource } from "../data-source";
import { defaultFilters } from "../filters";
import { createTauriDataSource, type Invoke } from "../tauri-data-source";

const FILTERS = defaultFilters("USD");

const SPEC = {
  range: { kind: "custom", first: "2026-05-01", last: "2026-05-30" },
  scope: { kind: "project", project: "tokenmtr" },
  format: "csv",
  currency: "USD",
} as const;

/** A data source whose only command is `command`, answering with `__fixtures__/<fixture>.json`. */
function serve(command: string, fixture: string = command): DataSource {
  const body: unknown = JSON.parse(
    readFileSync(join(import.meta.dirname, "__fixtures__", `${fixture}.json`), "utf8"),
  );
  const invoke: Invoke = (called) => {
    expect(called).toBe(command);
    return Promise.resolve(body);
  };
  return createTauriDataSource(invoke);
}

describe("golden command responses", () => {
  it("app_state (onboarding)", async () => {
    const state = await serve("app_state").appState();
    expect(state.kind).toBe("onboarding");
  });

  it("app_state (ready)", async () => {
    expect((await serve("app_state", "app_state.ready").appState()).kind).toBe("ready");
  });

  it("app_state (daemon_offline)", async () => {
    const state = await serve("app_state", "app_state.daemon_offline").appState();
    expect(state.kind).toBe("daemon_offline");
  });

  it("app_state (incompatible)", async () => {
    const state = await serve("app_state", "app_state.incompatible").appState();
    expect(state.kind).toBe("incompatible");
  });

  it("filter_options", async () => {
    expect((await serve("filter_options").filterOptions()).clients).toEqual(["Acme Corp"]);
  });

  it("settings", async () => {
    const settings = await serve("settings").settings();
    expect(settings.daemon.version).toBe("0.4.0");
    expect(settings.git).toEqual({ default_remote: "origin", default_branch: "main" });
    expect(settings.fx.latest?.eur_per_usd_scaled).toBe(881600000);
  });

  it("overview", async () => {
    const totals = await serve("overview").overview(FILTERS);
    expect(totals.api_value_micros).toBe(184523100);
    expect(totals.sessions).toBe(63);
  });

  it("series", async () => {
    const series = await serve("series").series(FILTERS, "project");
    expect(series.points[0]?.groups["(none)"]?.total_tokens).toBe(1231000);
  });

  it("models", async () => {
    const models = await serve("models").models(FILTERS);
    expect(models.currency).toBe("USD");
    expect(models.rows[1]?.divergent).toBe(true);
  });

  it("projects", async () => {
    const projects = await serve("projects").projects(FILTERS, "project");
    expect(projects.rows[1]?.client).toBe("Acme Corp");
  });

  it("project_models", async () => {
    const split = await serve("project_models").projectModels(FILTERS, "project");
    expect(split.rows[0]?.models[1]?.model).toBe("gpt-5");
    expect(split.rows[1]?.project).toBeNull();
  });

  it("project_remotes", async () => {
    const remotes = await serve("project_remotes").projectRemotes("github.com/ada/tokenmtr");
    expect(remotes.map((remote) => remote.keys_project)).toEqual([true, false]);
    expect(remotes[1]?.default_branch).toBeNull();
  });

  it("branches", async () => {
    expect((await serve("branches").branches(FILTERS)).rows[1]?.branch).toBeNull();
  });

  it("unresolved_cwds", async () => {
    expect(await serve("unresolved_cwds").unresolvedCwds(FILTERS)).toHaveLength(2);
  });

  it("heatmap", async () => {
    expect((await serve("heatmap").heatmap(FILTERS)).cells).toHaveLength(168);
  });

  it("tools", async () => {
    const tools = await serve("tools").tools(FILTERS);
    expect(tools.mcp_servers[0]?.tool_name).toBeNull();
    expect(tools.by_model[2]?.model).toBeNull();
  });

  it("signals", async () => {
    const signals = await serve("signals").signals(FILTERS);
    expect(signals.compactions).toBe(3);
    expect(signals.models[1]?.model).toBeNull();
  });

  it("billing", async () => {
    const billing = await serve("billing").billing(FILTERS);
    expect(billing.subscriptions[0]?.usage.events).toBe(1618);
  });

  it("billing_series", async () => {
    const series = await serve("billing_series").billingSeries(FILTERS, "daily", true);
    expect(series.points[1]?.savings_micros).toBe(-14570806);
    expect(series.points[2]?.roi).toBeNull();
  });

  it("sessions", async () => {
    const page = await serve("sessions").sessions(FILTERS, 0, 2);
    expect(page.total).toBe(63);
    expect(page.rows[1]?.project).toBeNull();
  });

  it("session_detail", async () => {
    const detail = await serve("session_detail").sessionDetail("claude", "0b7c1d2e", "USD");
    expect(detail.signals[1]).toEqual(["2026-05-03T13:30:02Z", "compaction"]);
  });

  it("session_origin", async () => {
    const origin = await serve("session_origin").sessionOrigin("claude", "0b7c1d2e");
    expect(origin.transcript_exists).toBe(true);
  });

  it("session_analytics", async () => {
    const analytics = await serve("session_analytics").sessionAnalytics(FILTERS);
    expect(analytics.rows[0]?.cache_hit_percent).toBe(97.5);
    expect(analytics.starts).toHaveLength(168);
    expect(analytics.histogram[0]?.models).toEqual([{ model: "gpt-5", sessions: 1 }]);
  });

  it("session_timeline", async () => {
    const timeline = await serve("session_timeline").sessionTimeline("claude", "0b7c1d2e", "USD");
    expect(timeline.signals[1]).toEqual({ at: "2026-05-03T13:30:02Z", kind: "compaction" });
    expect(timeline.models[0]?.api_value_micros).toBe(912300);
  });

  it("quota_latest", async () => {
    const meters = await serve("quota_latest").quotaLatest(FILTERS);
    expect(meters[0]?.windows[0]?.eta).toEqual({ kind: "will_hit", at: "2026-05-03T15:31:00Z" });
    expect(meters.map((meter) => meter.status)).toContain("keychain_denied");
  });

  it("quota_history", async () => {
    const history = await serve("quota_history").quotaHistory(FILTERS, "claude");
    expect(history[0]?.points).toHaveLength(3);
  });

  it("advisor", async () => {
    const reports = await serve("advisor").advisor(FILTERS);
    expect(reports.map((report) => report.advice.advice)).toEqual([
      "recommend",
      "recommend",
      "recommend",
      "recommend",
      "recommend",
      "unknown_quota",
      "not_enough_data",
      "no_plan",
    ]);
  });

  it("budgets", async () => {
    expect((await serve("budgets").budgets("USD"))[0]?.id).toBe("monthly-paid");
  });

  it("recap", async () => {
    const recap = await serve("recap").recap("today", "USD");
    expect(recap.tabs.map((tab) => tab.agent)).toEqual([null, "claude", "pi"]);
  });

  it("system_health", async () => {
    const health = await serve("system_health").systemHealth();
    expect(health.sinks.map((sink) => sink.state)).toEqual([
      "healthy",
      "failing",
      "lagging",
      "disabled",
    ]);
  });

  it("ingest_status", async () => {
    expect(await serve("ingest_status").ingestStatus()).toHaveLength(2);
  });

  it("parse_issues", async () => {
    expect((await serve("parse_issues").parseIssues()).map((issue) => issue.kind)).toEqual([
      "format_drift",
      "unknown_version",
      "protected_folder",
      "missing_cwd",
      "parse_error",
      "integration_drift",
    ]);
  });

  it("alerts", async () => {
    expect((await serve("alerts").alerts()).map((alert) => alert.kind)).toEqual([
      "quota_threshold",
      "quota_reset",
      "budget_threshold",
      "sink_failing",
      "format_drift",
      "backup_failed",
    ]);
  });

  it("backups", async () => {
    expect((await serve("backups").backups())[1]?.path).toBeNull();
  });

  it("fx_rates", async () => {
    expect((await serve("fx_rates").fxRates())[0]?.source).toBe("ecb");
  });

  it("generate_report", async () => {
    expect((await serve("generate_report").generateReport(SPEC)).format).toBe("csv");
  });

  it("save_report", async () => {
    expect(await serve("save_report").saveReport(SPEC)).toMatch(/\.csv$/);
  });

  it("project_rule_for", async () => {
    const rule = await serve("project_rule_for").projectRuleFor("/Users/ada/x");
    expect(rule.name).toBe("tokenmtr");
  });
});
