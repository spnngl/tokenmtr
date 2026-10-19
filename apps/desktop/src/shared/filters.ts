import { z } from "zod";
import { CURRENCIES, type Currency, type IsoDate } from "./format";

/**
 * The filter model of the full app. It lives in the URL search params, so views are
 * shareable and restorable. `null` means "all" for the free-form dimensions.
 */

export const RANGE_PRESETS = ["today", "7d", "30d", "90d"] as const;
export const BILLING_MODES = ["all", "subscription", "api"] as const;

export type RangePreset = (typeof RANGE_PRESETS)[number];
export type BillingMode = (typeof BILLING_MODES)[number];

export type Range =
  | { readonly kind: "preset"; readonly preset: RangePreset }
  | { readonly kind: "custom"; readonly from: IsoDate; readonly to: IsoDate };

export interface Filters {
  readonly range: Range;
  readonly agent: string | null;
  readonly profile: string | null;
  readonly model: string | null;
  readonly project: string | null;
  readonly client: string | null;
  readonly billing: BillingMode;
  readonly currency: Currency;
}

/** A search param that was present but invalid; its default was used instead. */
export interface FilterIssue {
  readonly param: string;
  readonly value: string;
  readonly message: string;
}

export interface ParsedFilters {
  readonly filters: Filters;
  readonly issues: readonly FilterIssue[];
}

/** Defaults for a fresh view; `currency` comes from `display.currency`. */
export function defaultFilters(currency: Currency): Filters {
  return {
    range: { kind: "preset", preset: "30d" },
    agent: null,
    profile: null,
    model: null,
    project: null,
    client: null,
    billing: "all",
    currency,
  };
}

const rangeParamSchema = z.enum([...RANGE_PRESETS, "custom"]);
const dateSchema = z.iso.date();
const labelSchema = z.string().trim().min(1).max(200);
const billingSchema = z.enum(BILLING_MODES);
const currencySchema = z.enum(CURRENCIES);

/** True for a real calendar date written `YYYY-MM-DD`. */
export function isIsoDate(value: string): boolean {
  return dateSchema.safeParse(value).success;
}

/** Reads one param; absent gives `fallback`, invalid gives `fallback` plus an issue. */
function readParam<S extends z.ZodType, F>(
  params: URLSearchParams,
  param: string,
  schema: S,
  fallback: F,
  issues: FilterIssue[],
): z.output<S> | F {
  const value = params.get(param);
  if (value === null) {
    return fallback;
  }
  const result = schema.safeParse(value);
  if (result.success) {
    return result.data;
  }
  issues.push({ param, value, message: result.error.issues[0]?.message ?? "invalid value" });
  return fallback;
}

function readRange(params: URLSearchParams, fallback: Range, issues: FilterIssue[]): Range {
  const kind = readParam(params, "range", rangeParamSchema, null, issues);
  if (kind === null) {
    return fallback;
  }
  if (kind !== "custom") {
    return { kind: "preset", preset: kind };
  }
  const issuesBefore = issues.length;
  const from = readParam(params, "from", dateSchema, null, issues);
  const to = readParam(params, "to", dateSchema, null, issues);
  if (from === null || to === null) {
    if (issues.length === issuesBefore) {
      issues.push({ param: "range", value: "custom", message: "custom range needs from and to" });
    }
    return fallback;
  }
  if (from > to) {
    issues.push({ param: "range", value: `${from}..${to}`, message: "from is after to" });
    return fallback;
  }
  return { kind: "custom", from, to };
}

/**
 * Parses URL search params into filters. Unknown params are ignored. A present-but-invalid value
 * falls back to `defaults` and is reported in `issues`, never thrown: a stale shared link still
 * opens.
 */
export function parseFilters(
  params: URLSearchParams,
  defaults: Filters = defaultFilters("USD"),
): ParsedFilters {
  const issues: FilterIssue[] = [];
  const filters: Filters = {
    range: readRange(params, defaults.range, issues),
    agent: readParam(params, "agent", labelSchema, defaults.agent, issues),
    profile: readParam(params, "profile", labelSchema, defaults.profile, issues),
    model: readParam(params, "model", labelSchema, defaults.model, issues),
    project: readParam(params, "project", labelSchema, defaults.project, issues),
    client: readParam(params, "client", labelSchema, defaults.client, issues),
    billing: readParam(params, "billing", billingSchema, defaults.billing, issues),
    currency: readParam(params, "currency", currencySchema, defaults.currency, issues),
  };
  return { filters, issues };
}

function sameRange(a: Range, b: Range): boolean {
  switch (a.kind) {
    case "preset":
      return b.kind === "preset" && a.preset === b.preset;
    case "custom":
      return b.kind === "custom" && a.from === b.from && a.to === b.to;
    default:
      a satisfies never;
      throw new Error("unreachable range kind");
  }
}

const LABEL_KEYS = ["agent", "profile", "model", "project", "client"] as const;

/** Serializes filters to search params, omitting every value equal to its default. */
export function serializeFilters(
  filters: Filters,
  defaults: Filters = defaultFilters("USD"),
): URLSearchParams {
  const params = new URLSearchParams();
  if (!sameRange(filters.range, defaults.range)) {
    switch (filters.range.kind) {
      case "preset":
        params.set("range", filters.range.preset);
        break;
      case "custom":
        params.set("range", "custom");
        params.set("from", filters.range.from);
        params.set("to", filters.range.to);
        break;
      default:
        filters.range satisfies never;
    }
  }
  for (const key of LABEL_KEYS) {
    const value = filters[key];
    if (value !== null) {
      params.set(key, value);
    }
  }
  if (filters.billing !== defaults.billing) {
    params.set("billing", filters.billing);
  }
  if (filters.currency !== defaults.currency) {
    params.set("currency", filters.currency);
  }
  return params;
}
