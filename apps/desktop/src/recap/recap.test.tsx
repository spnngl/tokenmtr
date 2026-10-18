import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../shared/axe";
import { Recap, type RecapProps } from "./recap";
import { MODEL } from "./recap-fixtures";

afterEach(cleanup);

function renderRecap(overrides: Partial<RecapProps> = {}) {
  const props: RecapProps = {
    model: MODEL,
    currency: "USD",
    fx: { kind: "fixed", eurPerUsd: 0.9 },
    timeZone: "Europe/Paris",
    onPeriodChange: vi.fn(),
    onOpenDashboard: vi.fn(),
    onOpenSettings: vi.fn(),
    onScanNow: vi.fn(),
    ...overrides,
  };
  return { props, ...render(<Recap {...props} />) };
}

describe("Recap", () => {
  it("lists All first and moves between tabs with the arrow keys", async () => {
    const user = userEvent.setup();
    renderRecap();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual(["All", "Claude", "pi"]);
    expect(tabs[0]?.getAttribute("aria-selected")).toBe("true");

    await user.tab();
    expect(document.activeElement).toBe(tabs[0]);
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(tabs[1]);
    expect(tabs[1]?.getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{End}");
    expect(tabs[2]?.getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{ArrowRight}");
    expect(tabs[0]?.getAttribute("aria-selected")).toBe("true");
  });

  it("hides blocks an agent cannot report", async () => {
    const user = userEvent.setup();
    renderRecap();
    expect(screen.getAllByRole("region", { name: "Quota" }).length).toBeGreaterThan(0);

    await user.click(screen.getByRole("tab", { name: "pi" }));
    expect(screen.queryByRole("region", { name: "Quota" })).toBeNull();
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

    await user.click(screen.getByRole("tab", { name: "Claude" }));
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
    renderRecap({ model: { ...MODEL, tabs: [] } });
    expect(screen.getByText("No agents found")).toBeTruthy();
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("has no axe violations", async () => {
    const { container } = renderRecap();
    expect(await axeViolations(container)).toEqual([]);
  });
});
