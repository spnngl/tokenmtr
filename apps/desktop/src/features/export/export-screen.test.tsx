import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";
import type { SystemHealth } from "../../shared/schemas";
import type { Remote } from "../common/remote-view";
import { ExportScreen } from "./export-screen";
import { EXPORT_HEALTH, SINK_DISABLED, SINK_FAILING, SINK_HEALTHY } from "./fixtures";
import { datasetStatuses, looksLikeProtectedFolder } from "./model";

afterEach(cleanup);

function renderScreen(remote: Remote<SystemHealth>, onExportNow = vi.fn(() => Promise.resolve())) {
  return render(<ExportScreen remote={remote} onExportNow={onExportNow} timeZone="UTC" />);
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

  it("error retries", async () => {
    const onRetry = vi.fn();
    renderScreen({ kind: "error", message: "Store is locked", onRetry });
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("daemon-offline offers to start it", () => {
    renderScreen({ kind: "daemon-offline", onStartDaemon: vi.fn() });
    expect(screen.getByRole("button", { name: "Install / start daemon" })).toBeTruthy();
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

  it("Export now runs once and reports completion", async () => {
    const onExportNow = vi.fn(() => Promise.resolve());
    renderScreen({ kind: "ready", data: EXPORT_HEALTH }, onExportNow);
    await userEvent.setup().click(screen.getByRole("button", { name: "Export now" }));
    expect(onExportNow).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.getByText("Export finished.")).toBeTruthy());
  });

  it("shows a failed export and Retry on a failing sink", async () => {
    const onExportNow = vi.fn(() => Promise.reject(new Error("Daemon unreachable")));
    renderScreen({ kind: "ready", data: EXPORT_HEALTH }, onExportNow);
    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Retry export of documents-copy" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe("Daemon unreachable"));
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
