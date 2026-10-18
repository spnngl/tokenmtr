import type { ReactNode } from "react";
import { type Currency, formatMoney } from "../../shared/format";

/**
 * Query results carry money already converted to the result currency (`*_micros` plus a root
 * `currency`), so nothing is converted here: a rate of exactly 1 makes `formatMoney` format only.
 */
const AS_IS = { kind: "fixed", eurPerUsd: 1 } as const;

/** `1_234_560_000` micros in `"USD"` -> "$1,234.56". Rounds half away from zero. */
export function formatAmount(micros: number, currency: Currency): string {
  return formatMoney(micros, "1970-01-01", currency, AS_IS).text;
}

interface AmountProps {
  readonly micros: number;
  readonly currency: Currency;
  /** The row's `approximate` flag (estimates, unpriced events, daily aggregates, old FX rate). */
  readonly approximate?: boolean;
}

/** A converted money figure; an approximate one is marked in text, never by colour alone. */
export function Amount({ micros, currency, approximate = false }: AmountProps): ReactNode {
  const text = formatAmount(micros, currency);
  if (!approximate) {
    return <span className="tabular-nums">{text}</span>;
  }
  return (
    <span className="tabular-nums">
      <span aria-hidden>≈ </span>
      <span className="sr-only">approximately </span>
      {text}
    </span>
  );
}
