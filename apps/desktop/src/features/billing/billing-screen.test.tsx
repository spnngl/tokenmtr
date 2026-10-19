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
  isOverBudget,
  isProjectedOver,
  untrackedPlans,
} from "./model";

afterEach(cleanup);

function renderScreen(remote: Remote<BillingData>, onTrackPlan = vi.fn()) {
  return render(<BillingScreen remote={remote} onTrackPlan={onTrackPlan} valuePanel={null} />);
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

  it("populated shows a budget over 100% and the estimate label", async () => {
    const onTrackPlan = vi.fn();
    const { container } = renderScreen({ kind: "ready", data: BILLING_DATA }, onTrackPlan);
    const user = userEvent.setup();

    const over = screen.getByRole("meter", { name: "Monthly paid cost" });
    expect(within(over.parentElement ?? container).getByText("128%")).toBeTruthy();
    expect(screen.getByText(/Over budget by \$28\.00/)).toBeTruthy();
    expect(screen.getByText("Projected to exceed")).toBeTruthy();
    // Without a rate an EUR budget shows its limit only, never placeholder zeros as spend.
    expect(
      screen.getByText(/Limit €20\.00, .* Spend shows once the first EUR exchange rate/),
    ).toBeTruthy();
    expect(screen.queryByRole("meter", { name: "Daily paid cost" })).toBeNull();

    expect(screen.getAllByText("Estimate")).toHaveLength(BILLING_DATA.advisor.length);
    expect(screen.getByText(/needs 14 days of quota history and has 5/i)).toBeTruthy();
    expect(screen.getByText(/Not applicable/)).toBeTruthy();
    expect(screen.getByText(/Consider Max 20x/)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: /Track it: claude-code/ }));
    expect(onTrackPlan).toHaveBeenCalledWith({
      agent: "claude-code",
      profile: "default",
      plan: "max_5x",
    });
    expect(await axeViolations(container)).toEqual([]);
  });

  it("shows Claude extra usage as its own line of Paid, and hides it without any", () => {
    renderScreen({ kind: "ready", data: BILLING_DATA });
    const kpi = screen.getByText("Extra usage", { selector: "dt" }).parentElement;
    expect(within(kpi ?? document.body).getByText("$12.35")).toBeTruthy();
    expect(screen.getByText("API cost, fees and extra usage")).toBeTruthy();
    cleanup();

    const none = (u: BillingData["report"]["total"]) => ({ ...u, extra_usage_micros: 0 });
    renderScreen({
      kind: "ready",
      data: {
        ...BILLING_DATA,
        report: {
          ...BILLING_DATA.report,
          total: none(BILLING_DATA.report.total),
          months: BILLING_DATA.report.months.map((month) => ({ ...month, extra_usage_micros: 0 })),
        },
      },
    });
    expect(screen.queryByText("Extra usage")).toBeNull();
    expect(screen.getByText("API cost plus fees")).toBeTruthy();
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
