import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";
import type { Remote } from "../../shared/remote-view";
import type { SystemHealth } from "../../shared/schemas";
import { ExportScreen } from "./export-screen";
import {
  EXPORT_HEALTH,
  EXPORT_HEALTH_WITH_SOURCES,
  SINK_DISABLED,
  SINK_FAILING,
  SINK_HEALTHY,
} from "./fixtures";
import { datasetStatuses, looksLikeProtectedFolder } from "./model";

afterEach(cleanup);

function renderScreen(
  remote: Remote<SystemHealth>,
  onExportNow = vi.fn(),
  isExporting = false,
  onImportNow = vi.fn(),
) {
  return render(
    <ExportScreen
      remote={remote}
      onExportNow={onExportNow}
      isExporting={isExporting}
      onImportNow={onImportNow}
      isImporting={false}
      timeZone="UTC"
    />,
  );
}

describe("export model", () => {
  it("detects a refused protected folder only on failing sinks", () => {
    expect(looksLikeProtectedFolder(SINK_FAILING)).toBe(true);
    expect(looksLikeProtectedFolder({ ...SINK_FAILING, last_error: "connection refused" })).toBe(
      false,
    );
    expect(looksLikeProtectedFolder(SINK_HEALTHY)).toBe(false);
    expect(looksLikeProtectedFolder(SINK_DISABLED)).toBe(false);
  });

  it("keeps the newest run per dataset", () => {
    const statuses = datasetStatuses(EXPORT_HEALTH.sinks);
    expect(statuses.find((d) => d.id === "usage_events")?.lastRun?.finished_at).toBe(
      "2026-05-21T12:03:00Z",
    );
    expect(statuses.find((d) => d.id === "fx_rates")?.lastRun).toBeNull();
  });
});

describe("ExportScreen", () => {
  it("loading", () => {
    renderScreen({ kind: "loading" });
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
  });

  it("empty explains how to add a sink", () => {
    renderScreen({ kind: "ready", data: { ...EXPORT_HEALTH, sinks: [] } });
    expect(screen.getByText("No export sink configured")).toBeTruthy();
  });

  it("a source-only config is not empty", () => {
    renderScreen({ kind: "ready", data: { ...EXPORT_HEALTH_WITH_SOURCES, sinks: [] } });
    expect(screen.queryByText("No export sink configured")).toBeNull();
    expect(screen.getByRole("heading", { name: "Sources" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Sinks" })).toBeNull();
  });

  it("error retries", async () => {
    const onRetry = vi.fn();
    renderScreen({
      kind: "error",
      title: "Could not read the usage database",
      message: "Store is locked",
      onRetry,
    });
    expect(screen.getByText("Store is locked")).toBeTruthy();
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("populated shows every sink state, the protected-folder hint and the datasets", async () => {
    const { container } = renderScreen({ kind: "ready", data: EXPORT_HEALTH });
    for (const label of ["Healthy", "Lagging", "Failing", "Disabled"]) {
      expect(screen.getByText(label)).toBeTruthy();
    }
    expect(screen.getByText(/5,400 rows, oldest 2h/)).toBeTruthy();
    expect(screen.getByRole("note").textContent).toMatch(/Full Disk Access/);
    const failing = screen.getByRole("table", {
      name: "Run history of documents-copy",
      hidden: true,
    });
    expect(within(failing).getByText("Failed")).toBeTruthy();
    expect(screen.getByRole("table", { name: "Exported datasets", hidden: true })).toBeTruthy();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("shows every source state with its error and run history", async () => {
    const { container } = renderScreen({ kind: "ready", data: EXPORT_HEALTH_WITH_SOURCES });
    expect(screen.getByRole("heading", { name: "Sources" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "superset" })).toBeNull();
    expect(screen.getByText(/Failing since .*: database is locked/)).toBeTruthy();
    expect(screen.getByText(/8,420 events, 1,310 tool calls/)).toBeTruthy();
    expect(
      screen.getByText(/Switched off in the config; its imported rows still count/),
    ).toBeTruthy();
    const runs = screen.getByRole("table", { name: "Run history of workstation", hidden: true });
    expect(within(runs).getByText("Failed")).toBeTruthy();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("Import now runs the import", async () => {
    const onImportNow = vi.fn();
    renderScreen({ kind: "ready", data: EXPORT_HEALTH_WITH_SOURCES }, vi.fn(), false, onImportNow);
    await userEvent.setup().click(screen.getByRole("button", { name: "Import now" }));
    expect(onImportNow).toHaveBeenCalledOnce();
  });

  it("has no Import now without an enabled sqlite source", () => {
    renderScreen({ kind: "ready", data: EXPORT_HEALTH });
    expect(screen.queryByRole("button", { name: "Import now" })).toBeNull();
  });

  it("Export now and Retry on a failing sink run the export", async () => {
    const onExportNow = vi.fn();
    renderScreen({ kind: "ready", data: EXPORT_HEALTH }, onExportNow);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Export now" }));
    await user.click(screen.getByRole("button", { name: "Retry export of documents-copy" }));
    expect(onExportNow).toHaveBeenCalledTimes(2);
  });

  it("keeps Export now readable but pending while an export runs", () => {
    renderScreen({ kind: "ready", data: EXPORT_HEALTH }, vi.fn(), true);
    const button = screen.getByRole("button", { name: "Export now" });
    expect(button.hasAttribute("data-pending")).toBe(true);
  });

  it("notes a daemon that is not running", () => {
    renderScreen({
      kind: "ready",
      data: { ...EXPORT_HEALTH, daemon: { state: "stale", last_heartbeat: null, age_secs: null } },
    });
    expect(screen.getByText("Daemon not running")).toBeTruthy();
  });

  it("reaches Export now and Retry by keyboard", async () => {
    renderScreen({ kind: "ready", data: EXPORT_HEALTH });
    const user = userEvent.setup();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Export now" }));
    for (
      let i = 0;
      i < 20 && document.activeElement !== screen.getByRole("button", { name: /Retry export/ });
      i++
    ) {
      await user.tab();
    }
    expect(document.activeElement).toBe(screen.getByRole("button", { name: /Retry export/ }));
  });
});
