import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { bootstrapConfig } from "../shared/app-context";
import { AppProvider } from "../shared/app-provider";
import { axeViolations } from "../shared/axe";
import { createMockDataSource, type MockScenario } from "../shared/mock";
import { MOCK_TIME_ZONE, NOW_MS } from "../shared/mock/seed";
import type { RecapSnapshot } from "../shared/schemas";
import { ThemeProvider } from "../shared/theme";
import { Recap, type RecapProps } from "./recap";
import { RecapApp } from "./recap-app";

afterEach(cleanup);

let snapshot: RecapSnapshot;

beforeAll(async () => {
  snapshot = await createMockDataSource().recap("today", "USD");
});

function renderRecap(overrides: Partial<RecapProps> = {}) {
  const props: RecapProps = {
    snapshot,
    period: "today",
    timeZone: MOCK_TIME_ZONE,
    now: NOW_MS,
    onPeriodChange: vi.fn(),
    onOpenDashboard: vi.fn(),
    onOpenSettings: vi.fn(),
    onScanNow: vi.fn(),
    isScanning: false,
    agentLabels: new Map([["claude", "Claude Code"]]),
    ...overrides,
  };
  return { props, ...render(<Recap {...props} />) };
}

describe("Recap", () => {
  it("lists All first and moves between tabs with the arrow keys", async () => {
    const user = userEvent.setup();
    renderRecap();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual(["All", "Claude Code", "pi", "aider"]);
    expect(tabs[0]?.getAttribute("aria-selected")).toBe("true");

    await user.tab();
    expect(document.activeElement).toBe(tabs[0]);
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(tabs[1]);
    expect(tabs[1]?.getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{End}");
    expect(tabs[3]?.getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{ArrowRight}");
    expect(tabs[0]?.getAttribute("aria-selected")).toBe("true");
  });

  it("shows quota meters with the ETA on the All tab", () => {
    renderRecap();
    const quota = screen.getByRole("region", { name: "Quota" });
    expect(within(quota).getByText(/100% at 15:40 at this pace/)).toBeTruthy();
    expect(within(quota).getByText(/resets in 2h 15m/)).toBeTruthy();
  });

  it("shows the monthly extra-usage meter with spend and cap, and none when it is off", async () => {
    renderRecap();
    const quota = screen.getByRole("region", { name: "Quota" });
    const extra = within(quota).getByRole("meter", { name: "Extra usage" });
    expect(extra.textContent).toMatch(/\$\d+\.\d\d of \$50\.00 this month/);
    cleanup();

    const off = await createMockDataSource({ scenario: "no-extra-usage" }).recap("today", "USD");
    renderRecap({ snapshot: off });
    const plain = screen.getByRole("region", { name: "Quota" });
    expect(within(plain).queryByRole("meter", { name: "Extra usage" })).toBeNull();
    expect(within(plain).getAllByRole("meter").length).toBeGreaterThan(0);
  });

  it("shows projected spend and the footer health with its reason", () => {
    renderRecap();
    const money = screen.getByRole("region", { name: "Money" });
    expect(within(money).getByText("Projected spend")).toBeTruthy();
    expect(screen.getByText("1 sink failing")).toBeTruthy();
  });

  it("shows the Live block only where the data has it", async () => {
    const user = userEvent.setup();
    renderRecap();
    await user.click(screen.getByRole("tab", { name: "Claude Code" }));
    const live = screen.getByRole("region", { name: "Live" });
    expect(live.textContent).toContain("12.5K");
    expect(live.textContent).toContain("$3.20");

    await user.click(screen.getByRole("tab", { name: "pi" }));
    expect(screen.queryByRole("region", { name: "Live" })).toBeNull();
  });

  it("flags a budget over 100% with its true percentage", async () => {
    const user = userEvent.setup();
    renderRecap();
    await user.click(screen.getByRole("tab", { name: "Claude Code" }));
    const budgets = screen.getByRole("region", { name: "Budgets" });
    expect(within(budgets).getByText(/^1\d\d%$/)).toBeTruthy();
  });

  it("an agent with every capability off shows no quota block and no error", async () => {
    const user = userEvent.setup();
    renderRecap();
    await user.click(screen.getByRole("tab", { name: "aider" }));
    expect(screen.queryByRole("region", { name: "Quota" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("region", { name: "Money" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "Tokens" })).toBeTruthy();
  });

  it("reports period changes", async () => {
    const user = userEvent.setup();
    const { props } = renderRecap();
    await user.click(screen.getByRole("radio", { name: "Month" }));
    expect(props.onPeriodChange).toHaveBeenCalledWith("month");
  });

  it("opens the dashboard unfiltered on All and filtered on an agent", async () => {
    const user = userEvent.setup();
    const { props } = renderRecap();
    await user.click(screen.getByRole("button", { name: "Open dashboard" }));
    expect(props.onOpenDashboard).toHaveBeenLastCalledWith(null);

    await user.click(screen.getByRole("tab", { name: "Claude Code" }));
    await user.click(screen.getByRole("button", { name: "Open dashboard" }));
    expect(props.onOpenDashboard).toHaveBeenLastCalledWith("claude");
  });

  it("wires Scan now and Settings", async () => {
    const user = userEvent.setup();
    const { props } = renderRecap();
    await user.click(screen.getByRole("button", { name: "Scan now" }));
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(props.onScanNow).toHaveBeenCalledOnce();
    expect(props.onOpenSettings).toHaveBeenCalledOnce();
  });

  it("shows the empty state when no agent exists", () => {
    renderRecap({ snapshot: { ...snapshot, tabs: [] } });
    expect(screen.getByText("No agents found")).toBeTruthy();
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("has no axe violations", async () => {
    const { container } = renderRecap();
    expect(await axeViolations(container)).toEqual([]);
  });
});

function renderApp(
  scenario: MockScenario,
  onAction?: (action: string, args: readonly unknown[]) => void,
) {
  const dataSource = createMockDataSource({
    scenario,
    ...(onAction === undefined ? {} : { onAction }),
  });
  const config = { ...bootstrapConfig, timeZone: MOCK_TIME_ZONE, now: () => NOW_MS };
  return render(
    <ThemeProvider>
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
      >
        <AppProvider config={config} dataSource={dataSource}>
          <RecapApp />
        </AppProvider>
      </QueryClientProvider>
    </ThemeProvider>,
  );
}

describe("RecapApp", () => {
  it("loads the recap from the data source", async () => {
    renderApp("populated");
    expect(await screen.findByRole("tab", { name: "All" })).toBeTruthy();
  });

  it("shows loading, then an error with retry", async () => {
    renderApp("loading");
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
    cleanup();
    renderApp("error");
    expect((await screen.findByRole("alert")).textContent).toContain("database is locked");
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("shows the empty state before any usage exists", async () => {
    renderApp("empty");
    expect(await screen.findByText("No agents found")).toBeTruthy();
  });

  it("scans from the footer", async () => {
    const onAction = vi.fn();
    renderApp("populated", onAction);
    await userEvent.setup().click(await screen.findByRole("button", { name: "Scan now" }));
    expect(onAction).toHaveBeenCalledWith("scanNow", []);
  });
});
