import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Input, Label, TextField } from "react-aria-components";
import { useAppConfig, useDataSource } from "../shared/app-context";
import { EMPTY_FILTER_OPTIONS, type FilterOptions } from "../shared/data-source";
import {
  BILLING_MODES,
  type BillingMode,
  type Filters,
  isIsoDate,
  type RANGE_PRESETS,
  type Range,
} from "../shared/filters";
import { type Currency, formatDay } from "../shared/format";
import { SegmentedControl } from "../shared/segmented-control";
import { FilterSelect } from "./filter-select";
import { useFilters } from "./use-filters";

type RangeChoice = (typeof RANGE_PRESETS)[number] | "custom";

const RANGE_OPTIONS: readonly { value: RangeChoice; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "7d", label: "Last 7 days" },
  { value: "30d", label: "Last 30 days" },
  { value: "90d", label: "Last 90 days" },
  { value: "custom", label: "Custom" },
];

const BILLING_LABELS: Record<BillingMode, string> = {
  all: "All billing",
  subscription: "Subscription",
  api: "API",
};

const CURRENCY_OPTIONS: readonly { value: Currency; label: string }[] = [
  { value: "USD", label: "USD" },
  { value: "EUR", label: "EUR" },
];

type Dimension = "agent" | "profile" | "model" | "project" | "client";

const DIMENSIONS: readonly {
  key: Dimension;
  label: string;
  all: string;
  options: keyof FilterOptions;
}[] = [
  { key: "agent", label: "Agent", all: "All agents", options: "agents" },
  { key: "profile", label: "Profile", all: "All profiles", options: "profiles" },
  { key: "model", label: "Model", all: "All models", options: "models" },
  { key: "project", label: "Project", all: "All projects", options: "projects" },
  { key: "client", label: "Client", all: "All clients", options: "clients" },
];

/** Dimension values are never empty (see `parseFilters`), so "" is free to mean "all". */
const ALL = "";

function rangeChoice(range: Range): RangeChoice {
  return range.kind === "custom" ? "custom" : range.preset;
}

function DateField(props: {
  readonly label: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
}): ReactNode {
  return (
    <TextField
      type="date"
      value={props.value}
      onChange={(value) => {
        if (isIsoDate(value)) {
          props.onChange(value);
        }
      }}
      className="flex min-w-0 flex-col gap-1"
    >
      <Label className="text-xs font-medium text-fg-muted">{props.label}</Label>
      <Input className="min-h-9 rounded-md border border-border bg-surface px-3 text-sm tabular-nums" />
    </TextField>
  );
}

function CustomRangeFields(props: {
  readonly from: string;
  readonly to: string;
  /** Called with the pair in edit order; the caller sorts it. */
  readonly onChange: (first: string, second: string) => void;
}): ReactNode {
  return (
    <>
      <DateField
        label="From"
        value={props.from}
        onChange={(from) => props.onChange(from, props.to)}
      />
      <DateField label="To" value={props.to} onChange={(to) => props.onChange(props.from, to)} />
    </>
  );
}

/** Filters bound to the URL search params (PLAN §7): shareable and restorable views. */
export function FilterBar(): ReactNode {
  const { filters, issues, setFilters } = useFilters();
  const { timeZone } = useAppConfig();
  const dataSource = useDataSource();
  const { data } = useQuery({
    queryKey: ["filter-options"],
    queryFn: () => dataSource.filterOptions(),
  });
  const options = data ?? EMPTY_FILTER_OPTIONS;
  const update = (patch: Partial<Filters>): void => setFilters({ ...filters, ...patch });

  const setRange = (choice: RangeChoice): void => {
    if (choice !== "custom") {
      update({ range: { kind: "preset", preset: choice } });
      return;
    }
    const today = formatDay(Date.now(), timeZone);
    update({
      range:
        filters.range.kind === "custom"
          ? filters.range
          : { kind: "custom", from: today, to: today },
    });
  };

  return (
    <form
      aria-label="Filters"
      onSubmit={(event) => event.preventDefault()}
      className="flex flex-col gap-2 border-b border-border bg-surface-raised p-3"
    >
      <div className="grid grid-cols-2 items-end gap-3 sm:flex sm:flex-wrap">
        <FilterSelect
          label="Range"
          options={RANGE_OPTIONS}
          value={rangeChoice(filters.range)}
          onChange={setRange}
        />
        {filters.range.kind === "custom" ? (
          <CustomRangeFields
            from={filters.range.from}
            to={filters.range.to}
            onChange={(first, second) =>
              update({
                range: {
                  kind: "custom",
                  from: first <= second ? first : second,
                  to: first <= second ? second : first,
                },
              })
            }
          />
        ) : null}
        {DIMENSIONS.map(({ key, label, all, options: optionsKey }) => {
          const current = filters[key];
          const values =
            current === null || options[optionsKey].includes(current)
              ? options[optionsKey]
              : [current, ...options[optionsKey]];
          return (
            <FilterSelect
              key={key}
              label={label}
              options={[
                { value: ALL, label: all },
                ...values.map((value) => ({ value, label: value })),
              ]}
              value={current ?? ALL}
              onChange={(value) => update({ [key]: value === ALL ? null : value })}
            />
          );
        })}
        <FilterSelect
          label="Billing"
          options={BILLING_MODES.map((value) => ({ value, label: BILLING_LABELS[value] }))}
          value={filters.billing}
          onChange={(billing) => update({ billing })}
        />
        <div className="flex flex-col gap-1">
          <span className="text-xs font-medium text-fg-muted">Currency</span>
          <SegmentedControl
            label="Currency"
            options={CURRENCY_OPTIONS}
            value={filters.currency}
            onChange={(currency) => update({ currency })}
          />
        </div>
      </div>
      {issues.length === 0 ? null : (
        <p role="status" className="text-xs text-fg-muted">
          Ignored invalid filters in the link: {issues.map((issue) => issue.param).join(", ")}.
        </p>
      )}
    </form>
  );
}
