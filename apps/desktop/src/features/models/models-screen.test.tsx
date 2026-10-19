import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";
import type { Remote } from "../../shared/remote-view";
import type { Series } from "../../shared/schemas";
import { costSeries } from "../insights/fixtures";
import { MODELS_REPORT } from "./fixtures";
import type { ModelsData } from "./model";
import { ModelsScreen } from "./models-screen";

afterEach(cleanup);

const SERIES: Remote<Series> = { kind: "ready", data: costSeries(7) };

function renderScreen(
  remote: Remote<ModelsData>,
  onAddOverride = vi.fn(),
  series: Remote<Series> = SERIES,
) {
  return render(<ModelsScreen remote={remote} series={series} onAddOverride={onAddOverride} />);
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

  it("draws every per-model chart, each with its table", async () => {
    const { container } = renderScreen({ kind: "ready", data: MODELS_REPORT });
    const user = userEvent.setup();
    for (const name of [
      "API value per model over time",
      "Cost share",
      "Cost per million tokens",
      "Token mix",
      "Cache hit rate",
    ]) {
      const figure = screen.getByRole("figure", { name });
      await user.click(within(figure).getByText("View as table"));
      expect(within(figure).getByRole("table")).toBeTruthy();
    }
    const share = screen.getByRole("table", { name: "Cost share per model" });
    expect(within(share).getByText("71.4%")).toBeTruthy();
    const perMillion = screen.getByRole("table", { name: "Cost per million tokens per model" });
    // The unpriced model has tokens but no value: its cost per token is zero, not missing.
    expect(within(perMillion).getAllByRole("row")).toHaveLength(MODELS_REPORT.rows.length + 1);
    expect(await axeViolations(container)).toEqual([]);
  });

  it("keeps the models when the cost series fails, with its own retry", async () => {
    const onRetry = vi.fn();
    renderScreen({ kind: "ready", data: MODELS_REPORT }, vi.fn(), {
      kind: "error",
      title: "Could not read the usage database",
      message: "Store is locked",
      onRetry,
    });
    expect(screen.getByRole("table", { name: "Usage per model" })).toBeTruthy();
    expect(screen.getByRole("figure", { name: "Cost share" })).toBeTruthy();
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("reaches every control by keyboard", async () => {
    renderScreen({ kind: "ready", data: MODELS_REPORT });
    const user = userEvent.setup();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Add price override" }));
    const table = screen.getByRole("region", { name: "Usage per model" });
    for (let step = 0; step < 40 && document.activeElement !== table; step += 1) {
      await user.tab();
    }
    expect(document.activeElement).toBe(table);
  });
});
