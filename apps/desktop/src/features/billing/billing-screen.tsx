import type { ReactNode } from "react";
import { type Remote, RemoteView } from "../../shared/remote-view";
import { PageHeader } from "../../shared/ui";
import { AdvisorPanel } from "./advisor-panel";
import { BudgetsPanel } from "./budgets-panel";
import { type BillingData, isBillingEmpty, type UntrackedPlan } from "./model";
import { SubscriptionsPanel } from "./subscriptions-panel";

interface BillingScreenProps {
  readonly remote: Remote<BillingData>;
  readonly onTrackPlan: (plan: UntrackedPlan) => void;
  /** The "Paid against API value" panel; it fetches its own range, so the route supplies it. */
  readonly valuePanel: ReactNode;
}

/** The Billing page: subscriptions, paid against API value, budgets and the plan advisor. */
export function BillingScreen({ remote, onTrackPlan, valuePanel }: BillingScreenProps): ReactNode {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader to="/billing" />
      <RemoteView
        remote={remote}
        emptyWhen={(data) =>
          isBillingEmpty(data)
            ? {
                title: "No billing data yet",
                hint: "Declare a subscription or a budget in Settings, then use an agent and run a scan.",
              }
            : null
        }
      >
        {(data) => (
          <div className="flex flex-col gap-8">
            <SubscriptionsPanel
              report={data.report}
              advisor={data.advisor}
              onTrackPlan={onTrackPlan}
            />
            {valuePanel}
            <BudgetsPanel budgets={data.budgets} />
            <AdvisorPanel reports={data.advisor} />
          </div>
        )}
      </RemoteView>
    </div>
  );
}
