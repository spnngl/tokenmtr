import { type ReactNode, useState } from "react";
import { BillingScreen } from "../../features/billing/billing-screen";
import type { UntrackedPlan } from "../../features/billing/model";
import { PaidVsValuePanel } from "../../features/billing/paid-vs-value-panel";
import { TrackPlanDialog } from "../../features/billing/track-plan-dialog";
import { useAdvisor, useBilling, useBudgets } from "../../shared/queries";
import { joinRemotes, toRemote } from "../../shared/use-remote";
import { useFilters } from "../use-filters";

/** Billing: subscriptions, paid against API value, budgets and the plan advisor. */
export function BillingRoute(): ReactNode {
  const { filters } = useFilters();
  const [tracking, setTracking] = useState<UntrackedPlan | null>(null);
  const report = toRemote(useBilling(filters));
  const budgets = toRemote(useBudgets(filters.currency));
  const advisor = toRemote(useAdvisor(filters));
  const remote = joinRemotes(
    joinRemotes(report, budgets, (report, budgets) => ({ report, budgets })),
    advisor,
    ({ report, budgets }, advisor) => ({ report, budgets, advisor }),
  );
  return (
    <>
      <BillingScreen remote={remote} onTrackPlan={setTracking} valuePanel={<PaidVsValuePanel />} />
      {tracking === null ? null : (
        <TrackPlanDialog
          detected={tracking}
          currency={filters.currency}
          onClose={() => setTracking(null)}
        />
      )}
    </>
  );
}
