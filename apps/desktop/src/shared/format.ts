/**
 * Display formatters. Every function is pure and deterministic: the locale is fixed and time
 * zones are always passed explicitly (D8), never read from the system.
 */

const LOCALE = "en-US";
const MICROS_PER_UNIT = 1_000_000;
const MICROS_PER_CENT = MICROS_PER_UNIT / 100;
const PLACEHOLDER = "\u2014";

export const CURRENCIES = ["USD", "EUR"] as const;
export type Currency = (typeof CURRENCIES)[number];

/** `YYYY-MM-DD`, a UTC calendar date. Lexicographic order is chronological order. */
export type IsoDate = string;

export interface FxRate {
  readonly date: IsoDate;
  readonly eurPerUsd: number;
}

/**
 * EUR-per-USD rates (D27).
 * - `fixed`: `fx.fixed_eur_per_usd` overrides the ECB; every figure is exact.
 * - `ecb`: `rates` sorted ascending by date. `offline` is true while the daemon cannot refresh
 *   them, so the table may end before the event date.
 */
export type Fx =
  | { readonly kind: "fixed"; readonly eurPerUsd: number }
  | { readonly kind: "ecb"; readonly rates: readonly FxRate[]; readonly offline: boolean };

export type MoneyText =
  | { readonly approximate: false; readonly text: string }
  | {
      readonly approximate: true;
      readonly text: string;
      /** `offline`: last known rate used; `before-first-rate`: earliest rate used; `no-rate`: none. */
      readonly reason: "offline" | "before-first-rate" | "no-rate";
    };

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

/** Latest rate dated strictly before `date`, plus whether it is the table's last entry. */
function rateBefore(
  rates: readonly FxRate[],
  date: IsoDate,
): { readonly rate: FxRate; readonly isLast: boolean } | undefined {
  let found: { readonly rate: FxRate; readonly isLast: boolean } | undefined;
  rates.forEach((rate, index) => {
    if (rate.date < date) {
      found = { rate, isLast: index === rates.length - 1 };
    }
  });
  return found;
}

/**
 * Formats a USD amount in the display currency.
 *
 * `amountUsdMicros` is a safe-integer `number`, not a `bigint`: the store keeps `i64`
 * micro-USD (D7) and Tauri's JSON bridge delivers it as a JSON number, which `bigint` could not
 * represent without a custom parser. 2^53 micro-USD is about 9 billion USD, far beyond any
 * usage total, and the Zod boundary schema rejects anything not a safe integer.
 *
 * EUR uses the latest rate dated strictly before `date` (D27), the event's UTC date. The result
 * is `approximate` when that rate is the last known one while rates cannot be refreshed, or
 * when `date` precedes every known rate.
 */
export function formatMoney(
  amountUsdMicros: number,
  date: IsoDate,
  currency: Currency,
  fx: Fx,
): MoneyText {
  if (currency === "USD") {
    const cents = roundHalfAwayFromZero(amountUsdMicros / MICROS_PER_CENT);
    return { approximate: false, text: formatCents(cents, "USD") };
  }
  if (fx.kind === "fixed") {
    const cents = roundHalfAwayFromZero((amountUsdMicros * fx.eurPerUsd) / MICROS_PER_CENT);
    return { approximate: false, text: formatCents(cents, "EUR") };
  }
  const before = rateBefore(fx.rates, date);
  const first = fx.rates[0];
  const used = before?.rate ?? first;
  if (used === undefined) {
    return { approximate: true, text: PLACEHOLDER, reason: "no-rate" };
  }
  const cents = roundHalfAwayFromZero((amountUsdMicros * used.eurPerUsd) / MICROS_PER_CENT);
  const text = formatCents(cents, "EUR");
  if (before === undefined) {
    return { approximate: true, text, reason: "before-first-rate" };
  }
  if (fx.offline && before.isLast) {
    return { approximate: true, text, reason: "offline" };
  }
  return { approximate: false, text };
}

const TOKEN_UNITS = ["", "K", "M", "B", "T"] as const;

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
 * Wall-clock hour bucket of an instant in `timeZone`, as `HH:00` (24h).
 * @throws {RangeError} when `timeZone` is not a valid IANA zone.
 */
export function formatHour(instantMs: number, timeZone: string): string {
  return `${zonedParts(instantMs, timeZone).hour}:00`;
}

/**
 * Wall-clock time of an instant in `timeZone`, as `HH:mm` (24h).
 * @throws {RangeError} when `timeZone` is not a valid IANA zone.
 */
export function formatClock(instantMs: number, timeZone: string): string {
  const { hour, minute } = zonedParts(instantMs, timeZone);
  return `${hour}:${minute}`;
}
