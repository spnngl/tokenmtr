import { type ReactNode, useState } from "react";
import { FilterSelect } from "../../app/filter-select";
import { useDataAction } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import type { SubscriptionSpec } from "../../shared/data-source";
import { FormDialog, FormTextField } from "../../shared/form-dialog";
import { CURRENCIES, type Currency, formatDay } from "../../shared/format";
import { type Plan, planSchema } from "../../shared/schemas";
import { planLabel, type UntrackedPlan } from "./model";

type Period = SubscriptionSpec["period"];

const PERIODS: readonly { readonly value: Period; readonly label: string }[] = [
  { value: "monthly", label: "Monthly" },
  { value: "yearly", label: "Yearly" },
];

const PLANS = planSchema.options.map((plan) => ({ value: plan, label: planLabel(plan) }));

const CURRENCY_OPTIONS = CURRENCIES.map((currency) => ({ value: currency, label: currency }));

/** A fee such as `20`, `20.5` or `20,50`: a non-negative amount with at most two decimals. */
const FEE_PATTERN = "\\d+([.,]\\d{1,2})?";

/** `"20.5"` -> 20_500_000 micro-units; `null` when the text is not a fee. */
export function parseFeeMicros(text: string): number | null {
  const match = /^(\d+)(?:[.,](\d{1,2}))?$/.exec(text.trim());
  if (match === null) {
    return null;
  }
  const whole = Number(match[1]);
  const cents = Number((match[2] ?? "").padEnd(2, "0"));
  const micros = whole * 1_000_000 + cents * 10_000;
  return Number.isSafeInteger(micros) ? micros : null;
}

/** The fields of the form, as typed. */
export interface TrackPlanForm {
  readonly plan: Plan;
  readonly fee: string;
  readonly currency: Currency;
  readonly period: Period;
  readonly start: string;
}

/**
 * The subscription for a detected plan: Anthropic traffic of Claude Code in the detected profile.
 * The id joins agent, profile and plan, so tracking the same profile's plan twice is refused by the
 * config. `null` when the fee is not a fee.
 */
export function subscriptionSpec(
  detected: UntrackedPlan,
  form: TrackPlanForm,
): SubscriptionSpec | null {
  const micros = parseFeeMicros(form.fee);
  if (micros === null) {
    return null;
  }
  return {
    id: `claude-${detected.profile}-${form.plan}`.replaceAll(/[^A-Za-z0-9._-]/g, "-").slice(0, 64),
    provider: "anthropic",
    plan: form.plan,
    agents: ["claude"],
    profiles: [detected.profile],
    fee: form.currency === "USD" ? { usd: micros } : { eur: micros },
    period: form.period,
    start: form.start,
    end: null,
  };
}

/**
 * Declares the subscription behind a plan detected for a Claude profile. Plan and profile come
 * from the detection; the fee is the user's to enter. Saving appends it to the config file.
 */
export function TrackPlanDialog(props: {
  readonly detected: UntrackedPlan;
  readonly currency: Currency;
  readonly onClose: () => void;
}): ReactNode {
  const { timeZone, now } = useAppConfig();
  const [form, setForm] = useState<TrackPlanForm>({
    plan: props.detected.plan,
    fee: "",
    currency: props.currency,
    period: "monthly",
    start: formatDay(now(), timeZone),
  });
  const add = useDataAction((ds, spec: SubscriptionSpec) => ds.addSubscription(spec), {
    failure: "Could not add the subscription",
    success: (_, spec) => `${planLabel(spec.plan)} subscription added`,
  });
  const update = (patch: Partial<TrackPlanForm>): void => setForm({ ...form, ...patch });
  return (
    <FormDialog
      title="Track subscription"
      isOpen
      onOpenChange={(open) => {
        if (!open) {
          props.onClose();
        }
      }}
      submitLabel="Add subscription"
      isPending={add.isPending}
      onSubmit={() => {
        const spec = subscriptionSpec(props.detected, form);
        if (spec !== null) {
          add.mutate(spec, { onSuccess: props.onClose });
        }
      }}
    >
      <p className="text-sm text-fg-muted">
        Anthropic plan of Claude Code, profile {props.detected.profile}. Its fee is weighed against
        the API value of the usage it covers.
      </p>
      <div className="flex flex-col gap-4 sm:flex-row">
        <FilterSelect
          label="Plan"
          options={PLANS}
          value={form.plan}
          onChange={(plan) => update({ plan })}
        />
        <FilterSelect
          label="Billing period"
          options={PERIODS}
          value={form.period}
          onChange={(period) => update({ period })}
        />
      </div>
      <div className="flex flex-col gap-4 sm:flex-row sm:items-start">
        <div className="sm:flex-1">
          <FormTextField
            label={form.period === "monthly" ? "Fee per month" : "Fee per year"}
            name="fee"
            value={form.fee}
            onChange={(fee) => update({ fee })}
            isRequired
            autoFocus
            inputMode="decimal"
            pattern={FEE_PATTERN}
            description="As billed, before tax: 100 or 21.60"
          />
        </div>
        <FilterSelect
          label="Currency"
          options={CURRENCY_OPTIONS}
          value={form.currency}
          onChange={(currency) => update({ currency })}
        />
      </div>
      <FormTextField
        label="Start date"
        name="start"
        value={form.start}
        onChange={(start) => update({ start })}
        isRequired
        pattern="\d{4}-\d{2}-\d{2}"
        description="First day covered, as YYYY-MM-DD"
      />
    </FormDialog>
  );
}
