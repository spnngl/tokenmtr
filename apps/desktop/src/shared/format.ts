/**
 * Display formatters. Every function is pure and deterministic: the locale is fixed and time
 * zones are always passed explicitly (D8), never read from the system.
 */

/** The one display locale (D8): figures and text ordering never depend on the machine. */
export const LOCALE = "en-US";
const MICROS_PER_UNIT = 1_000_000;
const MICROS_PER_CENT = MICROS_PER_UNIT / 100;
/** Shown where a value does not exist (no fee, no data), never `0`. */
export const PLACEHOLDER = "\u2014";

export const CURRENCIES = ["USD", "EUR"] as const;
export type Currency = (typeof CURRENCIES)[number];

/** `YYYY-MM-DD`, a UTC calendar date. Lexicographic order is chronological order. */
export type IsoDate = string;

const moneyFormats = new Map<Currency, Intl.NumberFormat>();

function moneyFormat(currency: Currency): Intl.NumberFormat {
  const cached = moneyFormats.get(currency);
  if (cached !== undefined) {
    return cached;
  }
  const created = new Intl.NumberFormat(LOCALE, {
    style: "currency",
    currency,
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
  moneyFormats.set(currency, created);
  return created;
}

/** Rounds half away from zero, so `-0.005` and `0.005` mirror each other. */
function roundHalfAwayFromZero(value: number): number {
  const rounded = Math.sign(value) * Math.round(Math.abs(value));
  return rounded === 0 ? 0 : rounded; // folds -0 into 0: never print "-$0.00"
}

function formatCents(cents: number, currency: Currency): string {
  return moneyFormat(currency).format(cents / 100);
}

/**
 * Formats an amount the backend already converted into `currency`: query results carry
 * `*_micros` plus the root `currency` they are expressed in, which is USD when EUR was asked for
 * but no FX rate exists (D27). Label money with that `currency`, never with the one requested.
 *
 * `micros` is a safe-integer `number`, not a `bigint`: the store keeps `i64` micro-units (D7)
 * and Tauri's JSON bridge delivers a JSON number. 2^53 micro-units is about 9 billion, far beyond
 * any usage total, and the Zod boundary schema rejects anything not a safe integer.
 *
 * The locale stays `en-US` for every currency ("€1,234.50"): one fixed locale keeps figures
 * deterministic across machines (D8).
 */
export function formatCurrencyMicros(micros: number, currency: Currency): string {
  return formatCents(roundHalfAwayFromZero(micros / MICROS_PER_CENT), currency);
}

const countFormat = new Intl.NumberFormat(LOCALE, { maximumFractionDigits: 0 });

/** A whole count with grouping: `12345` -> "12,345". */
export function formatCount(count: number): string {
  return Number.isFinite(count) ? countFormat.format(count) : PLACEHOLDER;
}

const TOKEN_UNITS = ["", "K", "M", "B", "T"] as const;

/** ROI is API value over fees paid: "4.0×"; `null` (no fee to compare with) is a dash. */
export function formatRoiMultiple(roi: number | null): string {
  return roi === null ? PLACEHOLDER : `${roi.toFixed(1)}×`;
}

/** `999` -> "999", `1_234` -> "1.2K", `3_400_000` -> "3.4M". One decimal, trailing `.0` dropped. */
export function formatTokens(count: number): string {
  if (!Number.isFinite(count)) {
    return PLACEHOLDER;
  }
  const sign = count < 0 ? "-" : "";
  let scaled = Math.abs(count);
  let unit = 0;
  while (unit < TOKEN_UNITS.length - 1 && Math.round(scaled * 10) / 10 >= 1000) {
    scaled /= 1000;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : 1;
  const text = new Intl.NumberFormat(LOCALE, {
    minimumFractionDigits: 0,
    maximumFractionDigits: digits,
  }).format(scaled);
  return `${sign}${text}${TOKEN_UNITS[unit] ?? ""}`;
}

/** `0.734` -> "73%". `fractionDigits` defaults to 0. Input is a fraction, not a percentage. */
export function formatPercent(fraction: number, fractionDigits = 0): string {
  if (!Number.isFinite(fraction)) {
    return PLACEHOLDER;
  }
  return new Intl.NumberFormat(LOCALE, {
    style: "percent",
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  }).format(fraction);
}

const DURATION_UNITS = [
  { label: "d", seconds: 86_400 },
  { label: "h", seconds: 3_600 },
  { label: "m", seconds: 60 },
  { label: "s", seconds: 1 },
] as const;

/** Two most significant units, a zero second unit dropped: 8_100 -> "2h 15m", 3_600 -> "1h". */
export function formatDuration(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) {
    return PLACEHOLDER;
  }
  let remaining = Math.floor(totalSeconds);
  const amounts = DURATION_UNITS.map((unit) => {
    const amount = Math.floor(remaining / unit.seconds);
    remaining -= amount * unit.seconds;
    return amount;
  });
  const start = amounts.findIndex((amount) => amount > 0);
  if (start < 0) {
    return "0s";
  }
  return [start, start + 1]
    .flatMap((index) => {
      const amount = amounts[index];
      const unit = DURATION_UNITS[index];
      return amount !== undefined && unit !== undefined && amount > 0
        ? [`${amount}${unit.label}`]
        : [];
    })
    .join(" ");
}

/** "resets in 2h 15m", "resets in <1m", or "resets now" once `resetsAtMs` has passed. */
export function formatRelativeReset(resetsAtMs: number, nowMs: number): string {
  const seconds = (resetsAtMs - nowMs) / 1000;
  if (seconds <= 0) {
    return "resets now";
  }
  if (seconds < 60) {
    return "resets in <1m";
  }
  return `resets in ${formatDuration(Math.floor(seconds / 60) * 60)}`;
}

const dateFormats = new Map<string, Intl.DateTimeFormat>();

function dateFormat(timeZone: string): Intl.DateTimeFormat {
  const cached = dateFormats.get(timeZone);
  if (cached !== undefined) {
    return cached;
  }
  const created = new Intl.DateTimeFormat("en-CA", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
  dateFormats.set(timeZone, created);
  return created;
}

function zonedParts(instantMs: number, timeZone: string): Record<string, string> {
  const parts: Record<string, string> = {};
  for (const part of dateFormat(timeZone).formatToParts(instantMs)) {
    parts[part.type] = part.value;
  }
  return parts;
}

/**
 * Calendar day of an instant in `timeZone` (an IANA name such as "Europe/Paris"), as
 * `YYYY-MM-DD`. This is the D8 bucketing key.
 * @throws {RangeError} when `timeZone` is not a valid IANA zone.
 */
export function formatDay(instantMs: number, timeZone: string): IsoDate {
  const { year, month, day } = zonedParts(instantMs, timeZone);
  return `${year}-${month}-${day}`;
}

/**
 * Wall-clock time of an instant in `timeZone`, as `HH:mm` (24h).
 * @throws {RangeError} when `timeZone` is not a valid IANA zone.
 */
export function formatClock(instantMs: number, timeZone: string): string {
  const { hour, minute } = zonedParts(instantMs, timeZone);
  return `${hour}:${minute}`;
}

/** `2026-06-15 14:05` in `timeZone`. */
export function formatDateTime(instantMs: number, timeZone: string): string {
  return `${formatDay(instantMs, timeZone)} ${formatClock(instantMs, timeZone)}`;
}
