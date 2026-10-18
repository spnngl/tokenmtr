import { z } from "zod";
import type { Filters } from "../filters";
import { currencySchema, dateSchema } from "./primitives";

/** `RangeSpec` of the query crate (tagged by `kind`; `custom.end` is exclusive). */
const rangeSpecSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("today") }),
  z.object({ kind: z.literal("last_days"), days: z.number().int().positive() }),
  z.object({ kind: z.literal("this_week") }),
  z.object({ kind: z.literal("this_month") }),
  z.object({ kind: z.literal("custom"), start: dateSchema, end: dateSchema }),
]);
type RangeSpec = z.infer<typeof rangeSpecSchema>;

/** `BillingFilter`: `subscription` is `{ subscription: null }` for any subscription. */
const billingFilterSchema = z.union([
  z.literal("any"),
  z.literal("api"),
  z.object({ subscription: z.string().nullable() }),
]);

/** `FilterSpec`: empty lists mean "no restriction". */
const filterSpecSchema = z.object({
  range: rangeSpecSchema,
  agents: z.array(z.string()),
  profiles: z.array(z.string()),
  models: z.array(z.string()),
  projects: z.array(z.string()),
  clients: z.array(z.string()),
  billing: billingFilterSchema,
});

/** What every filtered query command receives: the filters and the display currency. */
const queryRequestSchema = z.object({ filters: filterSpecSchema, currency: currencySchema });
export type QueryRequest = z.infer<typeof queryRequestSchema>;

const DAY_MS = 86_400_000;

/** The day after `date` (a UTC calendar date, so no zone is involved). */
function nextDay(date: string): string {
  return new Date(Date.parse(`${date}T00:00:00Z`) + DAY_MS).toISOString().slice(0, 10);
}

function rangeSpec(range: Filters["range"]): RangeSpec {
  if (range.kind === "custom") {
    // The filter bar's "to" is inclusive; the backend's `end` is exclusive.
    return { kind: "custom", start: range.from, end: nextDay(range.to) };
  }
  switch (range.preset) {
    case "today":
      return { kind: "today" };
    case "7d":
      return { kind: "last_days", days: 7 };
    case "30d":
      return { kind: "last_days", days: 30 };
    case "90d":
      return { kind: "last_days", days: 90 };
    default:
      range.preset satisfies never;
      throw new Error("unreachable range preset");
  }
}

function list(value: string | null): string[] {
  return value === null ? [] : [value];
}

/** The URL filters as the wire request. */
export function toQueryRequest(filters: Filters): QueryRequest {
  return {
    currency: filters.currency,
    filters: {
      range: rangeSpec(filters.range),
      agents: list(filters.agent),
      profiles: list(filters.profile),
      models: list(filters.model),
      projects: list(filters.project),
      clients: list(filters.client),
      billing:
        filters.billing === "all"
          ? "any"
          : filters.billing === "api"
            ? "api"
            : { subscription: null },
    },
  };
}
