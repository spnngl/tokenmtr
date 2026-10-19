import { z } from "zod";
import { RANGE_PRESETS, type Range } from "../shared/filters";

/**
 * The last global range the user chose, kept in `localStorage` (as JSON) so the next launch opens
 * on it. Only the range persists; the other filters start from their defaults.
 */
const STORAGE_KEY = "filters.range";

const dateSchema = z.iso.date();
const storedRangeSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("preset"), preset: z.enum(RANGE_PRESETS) }),
  z
    .object({ kind: z.literal("custom"), from: dateSchema, to: dateSchema })
    .refine((range) => range.from <= range.to, "from is after to"),
]);

/** The stored range, or `null` when absent, unreadable or invalid. */
export function readStoredRange(): Range | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) {
      return null;
    }
    const value: unknown = JSON.parse(raw);
    const parsed = storedRangeSchema.safeParse(value);
    return parsed.success ? parsed.data : null;
  } catch {
    return null; // storage blocked or not JSON: fall back to the default range
  }
}

/** Remembers `range` for the next launch. */
export function storeRange(range: Range): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(range));
  } catch {
    // storage blocked or full: the range simply does not persist
  }
}

let launch: Range | null | undefined;

/**
 * The range stored when this webview started, read once: every `useFilters` must share one
 * default, or a range equal to it would be dropped from the URL by one hook and read back as a
 * different default by another.
 */
export function launchRange(): Range | null {
  if (launch === undefined) {
    launch = readStoredRange();
  }
  return launch;
}
