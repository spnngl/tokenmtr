import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../../shared/axe";
import type { Remote } from "../../shared/remote-view";
import { BillingScreen } from "./billing-screen";
import { BILLING_DATA, BUDGET_OVER, BUDGET_PROJECTED } from "./fixtures";
import {
  type BillingData,
  budgetFraction,
  formatRoi,
  isOverBudget,
  isProjectedOver,
  untrackedPlans,
} from "./model";

afterEach(cleanup);

function renderScreen(remote: Remote<BillingData>, onTrackPlan = vi.fn()) {
  return render(<BillingScreen remote={remote} onTrackPlan={onTrackPlan} />);
}

const EMPTY: BillingData = {
  report: {
    currency: "USD",
    total: BILLING_DATA.report.total,
    api: { ...BILLING_DATA.report.api, events: 0 },
    subscriptions: [],
    months: [],
    roi: null,
  },
  budgets: [],
  advisor: [],
};

describe("billing model", () => {
  it("treats more than 100% as over budget and keeps the true fraction", () => {
    expect(isOverBudget(BUDGET_OVER)).toBe(true);
    expect(budgetFraction(BUDGET_OVER)).toBe(1.28);
    expect(isOverBudget(BUDGET_PROJECTED)).toBe(false);
    expect(isProjectedOver(BUDGET_PROJECTED)).toBe(true);
    expect(isProjectedOver(BUDGET_OVER)).toBe(false);
  });

  it("formats ROI and its absence", () => {
    expect(formatRoi(2.84)).toBe("2.8x");
    expect(formatRoi(null)).toBe("n/a");
  });

  it("finds the detected plan no subscription covers", () => {
    expect(untrackedPlans(BILLING_DATA.advisor, BILLING_DATA.report.subscriptions)).toEqual([
      { agent: "claude-code", profile: "default", plan: "max_5x" },
    ]);
  });
});

describe("BillingScreen", () => {
  it("loading", () => {
    renderScreen({ kind: "loading" });
    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
  });

  it("empty names the first step", () => {
    renderScreen({ kind: "ready", data: EMPTY });
    expect(screen.getByText("No billing data yet")).toBeTruthy();
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

  it("populated shows a budget over 100%, the estimate label and the monthly table", async () => {
    const onTrackPlan = vi.fn();
    const { container } = renderScreen({ kind: "ready", data: BILLING_DATA }, onTrackPlan);
    const user = userEvent.setup();

    const over = screen.getByRole("meter", { name: "Monthly paid cost" });
    expect(within(over.parentElement ?? container).getByText("128%")).toBeTruthy();
    expect(screen.getByText(/Over budget by \$28\.00/)).toBeTruthy();
    expect(screen.getByText("Projected to exceed")).toBeTruthy();

    expect(screen.getAllByText("Estimate")).toHaveLength(BILLING_DATA.advisor.length);
    expect(screen.getByText(/needs 14 days of quota history and has 5/i)).toBeTruthy();
    expect(screen.getByText(/Not applicable/)).toBeTruthy();
    expect(screen.getByText(/Consider Max 20x/)).toBeTruthy();

    const months = screen.getByRole("table", { name: "Billing per month", hidden: true });
    expect(within(months).getAllByRole("row")).toHaveLength(BILLING_DATA.report.months.length + 1);
    expect(
      screen
        .getByRole("figure", { name: "Paid and API value per month" })
        .getAttribute("data-points"),
    ).toBe("3");

    await user.click(screen.getByRole("button", { name: /Track it: claude-code/ }));
    expect(onTrackPlan).toHaveBeenCalledWith({
      agent: "claude-code",
      profile: "default",
      plan: "max_5x",
    });
    expect(await axeViolations(container)).toEqual([]);
  });

  it("reaches the track-it button by keyboard", async () => {
    renderScreen({ kind: "ready", data: BILLING_DATA });
    const user = userEvent.setup();
    await user.tab();
    const focusedInTable = document.activeElement?.closest(
      "section[aria-label='Declared subscriptions']",
    );
    expect(focusedInTable).toBeTruthy();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: /Track it/ }));
  });
});
