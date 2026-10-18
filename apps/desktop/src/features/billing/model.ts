import type {
  AdvisorReport,
  BillingReport,
  Budget,
  Plan,
  SubscriptionLine,
} from "../../shared/schemas";

/** Everything the Billing screen renders. */
export interface BillingData {
  readonly report: BillingReport;
  readonly budgets: readonly Budget[];
  readonly advisor: readonly AdvisorReport[];
}

/** True when there is nothing to bill, budget or advise on. */
export function isBillingEmpty(data: BillingData): boolean {
  return (
    data.report.subscriptions.length === 0 &&
    data.report.months.length === 0 &&
    data.report.api.events === 0 &&
    data.budgets.length === 0 &&
    data.advisor.length === 0
  );
}

const PLAN_LABELS: Record<Plan, string> = {
  pro: "Pro",
  max_5x: "Max 5x",
  max_20x: "Max 20x",
  team: "Team",
  enterprise: "Enterprise",
  custom: "Custom",
};

export function planLabel(plan: Plan): string {
  return PLAN_LABELS[plan];
}

/** API value over fees as "3.2x"; `null` (no fee accrued) is "n/a". */
export function formatRoi(roi: number | null): string {
  return roi === null ? "n/a" : `${roi.toFixed(1)}x`;
}

/** A budget's spend as a fraction of its limit; above 1 when over budget. */
export function budgetFraction(budget: Budget): number {
  return budget.percent / 100;
}

export function isOverBudget(budget: Budget): boolean {
  return budget.percent > 100;
}

/** Whether the projection to the period's end crosses the limit before the spend does. */
export function isProjectedOver(budget: Budget): boolean {
  return !isOverBudget(budget) && budget.projected_percent > 100;
}

/** A plan the advisor detected for an agent and no tracked subscription covers. */
export interface UntrackedPlan {
  readonly agent: string;
  readonly profile: string;
  readonly plan: Plan;
}

/**
 * Detected plans without a declared subscription of the same plan. Providers and agents are
 * different vocabularies, so the match is on the plan only; a false "covered" is possible when two
 * providers share a plan name, a false "untracked" is not.
 */
export function untrackedPlans(
  advisor: readonly AdvisorReport[],
  subscriptions: readonly SubscriptionLine[],
): readonly UntrackedPlan[] {
  const declared = new Set(subscriptions.map((line) => line.plan));
  const untracked: UntrackedPlan[] = [];
  for (const report of advisor) {
    if (report.current_plan !== null && !declared.has(report.current_plan)) {
      untracked.push({ agent: report.agent, profile: report.profile, plan: report.current_plan });
    }
  }
  return untracked;
}

/** Minimum snapshot history the advisor needs before it advises (D35). */
export const ADVISOR_MIN_DAYS = 14;
