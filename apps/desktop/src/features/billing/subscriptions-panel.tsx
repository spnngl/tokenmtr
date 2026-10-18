import type { ReactNode } from "react";
import { Button } from "../../shared/button";
import { formatCurrencyMicros } from "../../shared/format";
import type { AdvisorReport, BillingReport, SubscriptionLine } from "../../shared/schemas";
import { Amount, Badge, type Column, DataTable, Kpi, KpiGrid, Section } from "../../shared/ui";
import { formatRoi, planLabel, type UntrackedPlan, untrackedPlans } from "./model";

interface SubscriptionsPanelProps {
  readonly report: BillingReport;
  readonly advisor: readonly AdvisorReport[];
  /** Adds the detected plan to the config as a tracked subscription. */
  readonly onTrackPlan: (plan: UntrackedPlan) => void;
}

function columns(report: BillingReport): readonly Column<SubscriptionLine>[] {
  return [
    {
      header: "Subscription",
      cell: (line) => (
        <span className="flex flex-col">
          <span className="font-medium">{line.provider}</span>
          <span className="text-xs text-fg-muted">{planLabel(line.plan)}</span>
        </span>
      ),
    },
    {
      header: "Fee",
      numeric: true,
      cell: (line) =>
        `${formatCurrencyMicros(line.fee_micros, line.fee_currency)} / ${line.period === "monthly" ? "month" : "year"}`,
    },
    { header: "Since", cell: (line) => line.start },
    {
      header: "Fees in range",
      numeric: true,
      cell: (line) => (
        <Amount
          micros={line.usage.subscription_fee_micros}
          currency={report.currency}
          approximate={line.usage.approximate}
        />
      ),
    },
    {
      header: "API value covered",
      numeric: true,
      cell: (line) => (
        <Amount
          micros={line.usage.subscription_value_micros}
          currency={report.currency}
          approximate={line.usage.approximate}
        />
      ),
    },
    { header: "ROI", numeric: true, cell: (line) => formatRoi(line.roi) },
  ];
}

/** Headline figures, declared subscriptions, detected plans nobody tracks, and pay-as-you-go use. */
export function SubscriptionsPanel({
  report,
  advisor,
  onTrackPlan,
}: SubscriptionsPanelProps): ReactNode {
  const untracked = untrackedPlans(advisor, report.subscriptions);
  return (
    <Section
      title="Subscriptions"
      description="Declared subscriptions against the plans detected from your agents."
    >
      <KpiGrid>
        <Kpi
          label="Paid"
          value={
            <Amount
              micros={report.total.paid_micros}
              currency={report.currency}
              approximate={report.total.approximate}
            />
          }
          detail="API cost plus fees"
        />
        <Kpi
          label="API value"
          value={
            <Amount
              micros={report.total.api_value_micros}
              currency={report.currency}
              approximate={report.total.approximate}
            />
          }
          detail="What the usage costs at API rates"
        />
        <Kpi label="ROI" value={formatRoi(report.roi)} detail="API value over fees" />
        <Kpi
          label="Outside subscriptions"
          value={
            <Amount
              micros={report.api.api_value_micros}
              currency={report.currency}
              approximate={report.api.approximate}
            />
          }
          detail={`${report.api.events.toLocaleString("en-US")} pay-as-you-go events`}
        />
      </KpiGrid>
      {report.subscriptions.length === 0 ? (
        <p className="text-sm text-fg-muted">
          No subscription declared. Add one in Settings to see what your plan saves you.
        </p>
      ) : (
        <DataTable
          caption="Declared subscriptions"
          columns={columns(report)}
          rows={report.subscriptions}
          rowKey={(line) => line.id}
        />
      )}
      {untracked.length === 0 ? null : (
        <ul className="flex flex-col gap-2">
          {untracked.map((plan) => (
            <li
              key={`${plan.agent}/${plan.profile}`}
              className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border p-3 text-sm"
            >
              <span className="flex flex-wrap items-center gap-2">
                <Badge tone="warning">Detected, not tracked</Badge>
                <span>
                  {plan.agent} / {plan.profile} is on {planLabel(plan.plan)}, but no{" "}
                  {planLabel(plan.plan)} subscription is declared.
                </span>
              </span>
              <Button
                variant="secondary"
                aria-label={`Track it: ${plan.agent} / ${plan.profile} on ${planLabel(plan.plan)}`}
                onPress={() => onTrackPlan(plan)}
              >
                Track it
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}
