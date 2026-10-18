import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";
import type { Remote } from "../common/remote-view";
import { MODELS_REPORT } from "./fixtures";
import type { ModelsData } from "./model";
import { ModelsScreen } from "./models-screen";

afterEach(cleanup);

function renderScreen(remote: Remote<ModelsData>, onAddOverride = vi.fn()) {
  return render(<ModelsScreen remote={remote} onAddOverride={onAddOverride} />);
}

describe("ModelsScreen", () => {
  it("loading", () => {
    renderScreen({ kind: "loading" });
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
  });

  it("empty names the first step", () => {
    renderScreen({ kind: "ready", data: { currency: "USD", rows: [] } });
    expect(screen.getByText("No model usage yet")).toBeTruthy();
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

  it("populated flags unpriced and divergent models and keeps the table complete", async () => {
    const onAddOverride = vi.fn();
    const { container } = renderScreen({ kind: "ready", data: MODELS_REPORT }, onAddOverride);
    const table = screen.getByRole("table", { name: "Usage per model" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(MODELS_REPORT.rows.length + 1);
    expect(within(table).getByText("Unpriced")).toBeTruthy();
    expect(within(table).getByText("Diverges")).toBeTruthy();
    expect(within(table).getByText("-7.4%")).toBeTruthy();
    expect(within(table).getAllByText("approximately", { exact: false }).length).toBeGreaterThan(0);
    await userEvent.setup().click(screen.getByRole("button", { name: "Add price override" }));
    expect(onAddOverride).toHaveBeenCalledOnce();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("reaches every control by keyboard", async () => {
    renderScreen({ kind: "ready", data: MODELS_REPORT });
    const user = userEvent.setup();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Add price override" }));
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("region", { name: "Usage per model" }));
  });
});
