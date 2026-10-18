import type { ReactNode } from "react";
import { formatCurrencyMicros } from "../../shared/format";
import type { Budget, MeasuredBudget } from "../../shared/schemas";
import { Badge, Section } from "../../shared/ui";
import { UsageMeter } from "../../shared/usage-meter";
import { budgetFraction, isOverBudget, isProjectedOver } from "./model";

const PERIOD_LABELS = { day: "Daily", week: "Weekly", month: "Monthly" } as const;
const MEASURE_LABELS = { paid: "paid cost", api_value: "API value" } as const;

function budgetLabel(budget: Budget): string {
  return `${PERIOD_LABELS[budget.period]} ${MEASURE_LABELS[budget.measure]}`;
}

function MeasuredRow({ budget }: { readonly budget: MeasuredBudget }): ReactNode {
  const spent = formatCurrencyMicros(budget.spent_micros, budget.currency);
  const limit = formatCurrencyMicros(budget.limit_micros, budget.currency);
  const projected = formatCurrencyMicros(budget.projected_micros, budget.currency);
  return (
    <li className="flex flex-col gap-2 rounded-md border border-border bg-surface-raised p-3">
      <UsageMeter
        label={budgetLabel(budget)}
        usedFraction={budgetFraction(budget)}
        detail={`${spent} of ${limit}, ${budget.period_start} to ${budget.period_end}`}
      />
      <div className="flex flex-wrap items-center gap-2 text-sm">
        {isOverBudget(budget) ? (
          <Badge tone="danger">
            Over budget by{" "}
            {formatCurrencyMicros(budget.spent_micros - budget.limit_micros, budget.currency)}
          </Badge>
        ) : null}
        {isProjectedOver(budget) ? <Badge tone="warning">Projected to exceed</Badge> : null}
        <span className="text-fg-muted">
          Projected {projected} ({budget.projected_percent}%) by the end of the period.
        </span>
      </div>
    </li>
  );
}

function BudgetRow({ budget }: { readonly budget: Budget }): ReactNode {
  if (budget.state === "measured") {
    return <MeasuredRow budget={budget} />;
  }
  const limit = formatCurrencyMicros(budget.limit_micros, budget.currency);
  return (
    <li className="flex flex-col gap-1 rounded-md border border-border bg-surface-raised p-3 text-sm">
      <span className="font-medium">{budgetLabel(budget)}</span>
      <span className="text-fg-muted">
        Limit {limit}, {budget.period_start} to {budget.period_end}. Spend shows once the first{" "}
        {budget.currency} exchange rate is fetched.
      </span>
    </li>
  );
}

/** Spend against each budget, with the end-of-period projection. */
export function BudgetsPanel({ budgets }: { readonly budgets: readonly Budget[] }): ReactNode {
  return (
    <Section
      title="Budgets"
      description="Spend so far against each limit, and where it is heading."
    >
      {budgets.length === 0 ? (
        <p className="text-sm text-fg-muted">No budget set. Add one in Settings.</p>
      ) : (
        <ul className="flex flex-col gap-3">
          {budgets.map((budget) => (
            <BudgetRow key={budget.id} budget={budget} />
          ))}
        </ul>
      )}
    </Section>
  );
}
