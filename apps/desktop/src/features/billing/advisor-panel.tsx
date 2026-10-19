import type { ReactNode } from "react";
import { formatCount, formatCurrencyMicros } from "../../shared/format";
import type { Advice, AdvisorReport } from "../../shared/schemas";
import { Badge, type Column, DataTable, Section } from "../../shared/ui";
import { ADVISOR_MIN_DAYS, accountPlanLabel, planLabel } from "./model";

type Recommendation = Extract<Advice, { advice: "recommend" }>;
type PlanFit = Recommendation["fits"][number];

function verdictText(recommendation: Recommendation, current: string): string {
  const { verdict } = recommendation;
  switch (verdict.verdict) {
    case "keep":
      return `Keep ${current}: it fits your usage with room to spare.`;
    case "downgrade":
      return recommendation.current_monthly_fee === null
        ? `Consider ${planLabel(verdict.plan)}: your usage would fit its smaller quota.`
        : `Consider ${planLabel(verdict.plan)}: your usage would fit a cheaper plan.`;
    case "upgrade":
      return `Consider ${planLabel(verdict.plan)}: your usage is close to or above the current plan's limits.`;
    case "pay_as_you_go":
      return "Pay-as-you-go would likely cost less than any plan at your usage.";
    default:
      return verdict satisfies never;
  }
}

const FIT_COLUMNS: readonly Column<PlanFit>[] = [
  { header: "Plan", cell: (fit) => planLabel(fit.plan) },
  {
    header: "Fee / month",
    numeric: true,
    cell: (fit) => formatCurrencyMicros(fit.monthly_fee, "USD"),
  },
  { header: "5-hour peak", numeric: true, cell: (fit) => `${fit.five_hour_peak}%` },
  { header: "7-day peak", numeric: true, cell: (fit) => `${fit.seven_day_peak}%` },
  {
    header: "Fits",
    cell: (fit) =>
      fit.fits ? <Badge tone="success">Fits</Badge> : <Badge tone="warning">Too small</Badge>,
  },
];

/** How the current plan's quota was sized and priced, when that is not its own catalog entry. */
function sizingText(recommendation: Recommendation, current: string): string | null {
  const { sizing, current_monthly_fee: fee } = recommendation;
  const unknownFee =
    fee === null ? " Its fee is unknown: track it as a subscription to compare prices." : "";
  switch (sizing.source) {
    case "catalog":
      return null;
    case "rate_tier":
      return `${current}, quota sized as ${planLabel(sizing.plan)} from Claude's rate tier.${unknownFee}`;
    default:
      return sizing satisfies never;
  }
}

function AdviceBody({ report }: { readonly report: AdvisorReport }): ReactNode {
  const { advice } = report;
  switch (advice.advice) {
    case "not_enough_data":
      return (
        <p className="text-sm">
          Not enough data yet: the advisor needs {ADVISOR_MIN_DAYS} days of quota history and has{" "}
          {advice.span_days}.
        </p>
      );
    case "no_plan":
      return (
        <p className="text-sm">
          No plan detected: Claude has not reported a subscription for this account, so there is
          nothing to compare.
        </p>
      );
    case "unknown_quota":
      return (
        <p className="text-sm">
          {report.current_plan === null
            ? "This plan"
            : accountPlanLabel(report.current_plan, report.organization)}{" "}
          was detected, but its quota size is unknown: Claude's rate tier does not name a plan to
          compare with.
        </p>
      );
    case "recommend": {
      const label = accountPlanLabel(advice.current, report.organization);
      const sizing = sizingText(advice, label);
      return (
        <div className="flex flex-col gap-3">
          <p className="text-sm font-medium">{verdictText(advice, label)}</p>
          {sizing === null ? null : <p className="text-sm text-fg-muted">{sizing}</p>}
          <dl className="grid grid-cols-2 gap-2 text-sm md:grid-cols-3 lg:grid-cols-5">
            <Input label="Current plan" value={label} />
            <Input
              label="Current fee / month"
              value={
                advice.current_monthly_fee === null
                  ? "Unknown"
                  : formatCurrencyMicros(advice.current_monthly_fee, "USD")
              }
            />
            <Input
              label="Average month at API rates"
              value={formatCurrencyMicros(report.monthly_api_value_usd_micros, "USD")}
            />
            <Input label="Peak headroom" value={`${advice.headroom}%`} />
            <Input label="Quota snapshots" value={formatCount(report.snapshots)} />
          </dl>
          <DataTable
            caption={`Plan fit for ${report.agent} / ${report.profile}`}
            columns={FIT_COLUMNS}
            rows={advice.fits}
            rowKey={(fit) => fit.plan}
          />
        </div>
      );
    }
    default:
      return advice satisfies never;
  }
}

function Input({ label, value }: { readonly label: string; readonly value: string }): ReactNode {
  return (
    <div className="flex flex-col">
      <dt className="text-xs text-fg-muted">{label}</dt>
      <dd className="tabular-nums">{value}</dd>
    </div>
  );
}

/** The plan advisor: always labelled an estimate, with the inputs it used. */
export function AdvisorPanel({
  reports,
}: {
  readonly reports: readonly AdvisorReport[];
}): ReactNode {
  return (
    <Section
      title="Plan advisor"
      description="Compares your quota peaks and API value with each plan. A guide, not a guarantee."
    >
      {reports.length === 0 ? (
        <p className="text-sm text-fg-muted">No agent with quota data to advise on yet.</p>
      ) : (
        <ul className="flex flex-col gap-3">
          {reports.map((report) => (
            <li
              key={`${report.agent}/${report.profile}`}
              className="flex flex-col gap-3 rounded-md border border-border bg-surface-raised p-3"
            >
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-sm font-semibold">
                  {report.agent} / {report.profile}
                </h3>
                <Badge tone="neutral">Estimate</Badge>
              </div>
              <AdviceBody report={report} />
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}
