import type { ReactNode } from "react";
import { type Currency, type Fx, formatMoney, type IsoDate } from "./format";

interface MoneyProps {
  readonly usdMicros: number;
  /** UTC date of the event, or the period's end for an aggregate (D27). */
  readonly date: IsoDate;
  readonly currency: Currency;
  readonly fx: Fx;
}

/** A money figure through `formatMoney`; an approximate figure is marked in text, not colour. */
export function Money({ usdMicros, date, currency, fx }: MoneyProps): ReactNode {
  const money = formatMoney(usdMicros, date, currency, fx);
  if (!money.approximate) {
    return <span className="tabular-nums">{money.text}</span>;
  }
  return (
    <span className="tabular-nums">
      <span aria-hidden>≈ </span>
      <span className="sr-only">approximately </span>
      {money.text}
    </span>
  );
}
