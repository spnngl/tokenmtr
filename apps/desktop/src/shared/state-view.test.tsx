import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "./axe";
import { StateView, type ViewState } from "./state-view";

afterEach(cleanup);

function renderState(state: ViewState) {
  return render(
    <StateView state={state}>
      <p>Usage content</p>
    </StateView>,
  );
}

describe("StateView", () => {
  it("renders content for ready", () => {
    renderState({ kind: "ready" });
    expect(screen.getByText("Usage content")).toBeTruthy();
  });

  it("loading replaces content and is a busy status", () => {
    renderState({ kind: "loading" });
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
    expect(screen.queryByText("Usage content")).toBeNull();
  });

  it("empty names the first step", () => {
    renderState({ kind: "empty", title: "No agents found", hint: "Start a session." });
    expect(screen.getByText("No agents found")).toBeTruthy();
    expect(screen.getByText("Start a session.")).toBeTruthy();
    expect(screen.queryByText("Usage content")).toBeNull();
  });

  it("error is an alert with a working Retry", async () => {
    const onRetry = vi.fn();
    renderState({ kind: "error", message: "Store is locked", onRetry });
    expect(screen.getByRole("alert").textContent).toContain("Store is locked");
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("daemon-offline offers Install / start daemon", async () => {
    const onStartDaemon = vi.fn();
    renderState({ kind: "daemon-offline", onStartDaemon });
    await userEvent.setup().click(screen.getByRole("button", { name: "Install / start daemon" }));
    expect(onStartDaemon).toHaveBeenCalledOnce();
    expect(screen.queryByText("Usage content")).toBeNull();
  });

  it("incompatible names both versions and offers Restart daemon", async () => {
    const onRestartDaemon = vi.fn();
    renderState({
      kind: "incompatible",
      appVersion: "0.2.0",
      daemonVersion: "0.1.0",
      onRestartDaemon,
    });
    expect(screen.getByText("Update required")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("0.2.0");
    expect(screen.getByRole("alert").textContent).toContain("0.1.0");
    expect(screen.queryByText("Usage content")).toBeNull();
    await userEvent.setup().click(screen.getByRole("button", { name: "Restart daemon" }));
    expect(onRestartDaemon).toHaveBeenCalledOnce();
  });

  it("partial keeps the content and links to parse issues", () => {
    renderState({ kind: "partial", failedCollectors: 2, issuesHref: "#/settings" });
    expect(screen.getByText("Usage content")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("2 collectors failed");
    expect(screen.getByRole("link", { name: "View parse issues" }).getAttribute("href")).toBe(
      "#/settings",
    );
  });

  it("stale keeps the content and offers Scan now", async () => {
    const onScanNow = vi.fn();
    renderState({ kind: "stale", lastScan: "3 hours ago", onScanNow });
    expect(screen.getByText("Usage content")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Last scan 3 hours ago");
    await userEvent.setup().click(screen.getByRole("button", { name: "Scan now" }));
    expect(onScanNow).toHaveBeenCalledOnce();
  });

  it.each<ViewState>([
    { kind: "ready" },
    { kind: "loading" },
    { kind: "empty", title: "No agents found", hint: "Start a session." },
    { kind: "error", message: "Boom", onRetry: () => undefined },
    { kind: "daemon-offline", onStartDaemon: () => undefined },
    {
      kind: "incompatible",
      appVersion: "0.2.0",
      daemonVersion: "0.1.0",
      onRestartDaemon: () => undefined,
    },
    { kind: "partial", failedCollectors: 1, issuesHref: "#/settings" },
    { kind: "stale", lastScan: "1h ago", onScanNow: () => undefined },
  ])("has no axe violations for $kind", async (state) => {
    const { container } = renderState(state);
    expect(await axeViolations(container)).toEqual([]);
  });
});
