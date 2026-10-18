import type { ReactNode } from "react";
import { BillingScreen } from "../../features/billing/billing-screen";
import { joinRemotes, toRemote, useDaemonGate } from "../../features/common/use-remote";
import { useDataSource } from "../../shared/app-context";
import { useAdvisor, useBilling, useBudgets, useSettings } from "../../shared/queries";
import { useFilters } from "../use-filters";

/** Billing: subscriptions, paid against API value, budgets and the plan advisor. */
export function BillingRoute(): ReactNode {
  const { filters } = useFilters();
  const dataSource = useDataSource();
  const settings = useSettings();
  const gate = useDaemonGate();
  const report = toRemote(useBilling(filters), gate);
  const budgets = toRemote(useBudgets(filters.currency), gate);
  const advisor = toRemote(useAdvisor(filters), gate);
  const remote = joinRemotes(
    joinRemotes(report, budgets, (report, budgets) => ({ report, budgets })),
    advisor,
    ({ report, budgets }, advisor) => ({ report, budgets, advisor }),
  );
  return (
    <BillingScreen
      remote={remote}
      // Subscriptions live in the config file: show it where the user adds the entry.
      onTrackPlan={() => {
        if (settings.data !== undefined) {
          void dataSource.revealPath(settings.data.config_path);
        }
      }}
    />
  );
}
