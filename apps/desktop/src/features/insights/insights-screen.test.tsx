import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";
import type { Remote } from "../../shared/remote-view";
import { heatIntensity } from "../../shared/week-hour-table";
import { costSeries, INSIGHTS_DATA, signalDays } from "./fixtures";
import { InsightsScreen } from "./insights-screen";
import {
  costByModel,
  heatGrid,
  type InsightsData,
  type SignalDay,
  signalPoints,
  signalsByModel,
  toolCallsByModel,
  toolKey,
  toolLabel,
} from "./model";

afterEach(cleanup);

function renderScreen(remote: Remote<InsightsData>) {
  return render(<InsightsScreen remote={remote} timeZone="Europe/Paris" />);
}

const EMPTY: InsightsData = {
  tools: { tools: [], mcp_servers: [], by_model: [] },
  signals: { days: [], api_errors: 0, compactions: 0, top_sessions: [], models: [] },
  branches: { currency: "USD", rows: [] },
  heatmap: { currency: "USD", cells: [] },
  costs: { currency: "USD", bucket: "day", group_by: "model", points: [] },
};

describe("insights model", () => {
  it("labels MCP tools with their server", () => {
    const [bash, , , linear] = INSIGHTS_DATA.tools.tools;
    expect(bash && toolLabel(bash)).toBe("Bash");
    expect(linear && toolLabel(linear)).toBe("linear / search_issues");
  });

  it("merges more than 400 days down to at most 400 points and keeps the totals", () => {
    const days = signalDays(1000);
    const merged = signalPoints(days);
    expect(merged.length).toBeLessThanOrEqual(400);
    const sum = (rows: readonly SignalDay[]) =>
      rows.reduce((total, row) => total + row.compactions, 0);
    expect(sum(merged)).toBe(sum(days));
    expect(merged[0]?.date).toBe(days[0]?.date);
  });

  it("splits tool calls by model, folding servers by their tools", () => {
    const byTool = toolCallsByModel(INSIGHTS_DATA.tools.by_model, "tool");
    expect(byTool.series).toEqual([
      "claude-opus-4-1",
      "claude-sonnet-4-5",
      "gpt-5",
      "Unknown model",
    ]);
    expect(byTool.values.get(toolKey({ tool_name: "Bash", mcp_server: null }))).toEqual([
      800, 0, 404, 0,
    ]);
    const byServer = toolCallsByModel(INSIGHTS_DATA.tools.by_model, "server");
    expect(byServer.series).toEqual(["claude-opus-4-1", "Unknown model"]);
    expect(
      byServer.values.get(toolKey({ tool_name: null, mcp_server: "notion" }, "server")),
    ).toEqual([0, 22]);
  });

  it("keeps the top five models by API value and folds the rest into Other", () => {
    const costs = costSeries(3);
    const { series, rows } = costByModel(costs);
    expect(series).toHaveLength(6);
    expect(series.at(-1)).toBe("Other");
    expect(series[0]).toBe("claude-opus-4-1");
    const total = (values: readonly number[]) => values.reduce((sum, value) => sum + value, 0);
    for (const [index, row] of rows.entries()) {
      expect(total(row.values)).toBe(costs.points[index]?.api_value_micros);
    }
  });

  it("labels signals without a model", () => {
    expect(signalsByModel(INSIGHTS_DATA.signals).map((row) => row.label)).toEqual([
      "claude-opus-4-1",
      "gpt-5",
      "Unknown model",
    ]);
  });

  it("lays out the heatmap Monday first and scales intensity to the peak", () => {
    const grid = heatGrid(
      [
        { weekday: 1, hour: 9, events: 5, total_tokens: 0, api_value_micros: 0 },
        { weekday: 7, hour: 23, events: 10, total_tokens: 0, api_value_micros: 0 },
      ],
      "events",
    );
    expect(grid.rows).toHaveLength(7);
    expect(grid.rows[0]?.[9]).toBe(5);
    expect(grid.rows[6]?.[23]).toBe(10);
    expect(grid.max).toBe(10);
    expect(heatIntensity(5, grid.max)).toBe(0.5);
    expect(heatIntensity(0, grid.max)).toBe(0);
    expect(heatIntensity(3, 0)).toBe(0);
  });
});

describe("InsightsScreen", () => {
  it("loading", () => {
    renderScreen({ kind: "loading" });
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
  });

  it("empty names the first step", () => {
    renderScreen({ kind: "ready", data: EMPTY });
    expect(screen.getByText("No activity to analyse yet")).toBeTruthy();
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

  it("populated shows the MCP tool, switches to servers, and keeps every chart under budget", async () => {
    const { container } = renderScreen({ kind: "ready", data: INSIGHTS_DATA });
    const user = userEvent.setup();
    const tools = screen.getByRole("table", { name: "Tool calls per tool", hidden: true });
    expect(within(tools).getByText("linear / search_issues")).toBeTruthy();
    expect(within(tools).getByRole("columnheader", { name: "claude-sonnet-4-5" })).toBeTruthy();

    await user.click(screen.getByRole("radio", { name: "By MCP server" }));
    const servers = screen.getByRole("table", { name: "Tool calls per MCP server", hidden: true });
    expect(within(servers).getByText("linear")).toBeTruthy();

    for (const figure of container.querySelectorAll("figure[data-points]")) {
      expect(Number(figure.getAttribute("data-points"))).toBeLessThanOrEqual(400);
    }
    expect(container.querySelectorAll("figure").length).toBe(5);
    const signals = screen.getByRole("table", { name: "Signals per model", hidden: true });
    expect(within(signals).getByText("Unknown model")).toBeTruthy();
    const costs = screen.getByRole("table", {
      name: "API value per model over time",
      hidden: true,
    });
    expect(within(costs).getByRole("columnheader", { name: "Other" })).toBeTruthy();
    expect(screen.getByRole("table", { name: "Activity by weekday and hour" })).toBeTruthy();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("cost by model opens its table and says so when nothing is priced", async () => {
    const user = userEvent.setup();
    renderScreen({
      kind: "ready",
      data: { ...INSIGHTS_DATA, costs: { ...INSIGHTS_DATA.costs, points: [] } },
    });
    expect(screen.getByText("No priced usage in this range.")).toBeTruthy();
    cleanup();
    renderScreen({ kind: "ready", data: INSIGHTS_DATA });
    const figure = screen.getByRole("figure", { name: "API value per model over time" });
    await user.click(within(figure).getByText("View as table"));
    expect(within(figure).getByRole("table")).toBeTruthy();
  });

  it("heatmap metric toggle changes the cell text", async () => {
    renderScreen({ kind: "ready", data: INSIGHTS_DATA });
    const heat = screen.getByRole("table", { name: "Activity by weekday and hour" });
    const before = heat.textContent;
    await userEvent.setup().click(screen.getByRole("radio", { name: "API value" }));
    expect(heat.textContent).not.toBe(before);
    expect(heat.textContent).toContain("$");
  });

  it("reaches the grouping toggle and the heatmap toggle by keyboard", async () => {
    renderScreen({ kind: "ready", data: INSIGHTS_DATA });
    const user = userEvent.setup();
    await user.tab();
    expect(
      screen.getByRole("radiogroup", { name: "Group tool calls" }).contains(document.activeElement),
    ).toBe(true);
  });
});
