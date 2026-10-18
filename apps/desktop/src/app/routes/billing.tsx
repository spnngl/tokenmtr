import type { ReactNode } from "react";
import { BillingScreen } from "../../features/billing/billing-screen";
import { useRevealConfig } from "../../shared/actions";
import { useAdvisor, useBilling, useBudgets } from "../../shared/queries";
import { joinRemotes, toRemote } from "../../shared/use-remote";
import { useFilters } from "../use-filters";

/** Billing: subscriptions, paid against API value, budgets and the plan advisor. */
export function BillingRoute(): ReactNode {
  const { filters } = useFilters();
  const revealConfig = useRevealConfig();
  const report = toRemote(useBilling(filters));
  const budgets = toRemote(useBudgets(filters.currency));
  const advisor = toRemote(useAdvisor(filters));
  const remote = joinRemotes(
    joinRemotes(report, budgets, (report, budgets) => ({ report, budgets })),
    advisor,
    ({ report, budgets }, advisor) => ({ report, budgets, advisor }),
  );
  return (
    <BillingScreen
      remote={remote}
      // Subscriptions live in the config file: show it where the user adds the entry.
      onTrackPlan={() => revealConfig.mutate()}
    />
  );
}
