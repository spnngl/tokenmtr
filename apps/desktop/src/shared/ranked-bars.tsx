import type { ReactNode } from "react";
import { chartColor } from "./charts";

export interface RankedBar {
  readonly key: string;
  readonly label: ReactNode;
  /** Right-aligned figure, e.g. the formatted cost. */
  readonly value: ReactNode;
  /** 0-100: the bar length. */
  readonly percent: number;
  /** Small text under the label. */
  readonly detail?: ReactNode;
  readonly onPress?: () => void;
}

/**
 * A ranked list with a proportional bar behind each row. The list is the data (the bar is
 * decoration), so it needs no separate table alternative.
 */
export function RankedBars({
  items,
  label,
}: {
  readonly items: readonly RankedBar[];
  readonly label: string;
}): ReactNode {
  return (
    <ul aria-label={label} className="flex flex-col gap-2">
      {items.map((item, index) => {
        const body = (
          <>
            <span className="flex items-baseline justify-between gap-3 text-sm">
              <span className="min-w-0 truncate font-medium">{item.label}</span>
              <span className="shrink-0 tabular-nums">{item.value}</span>
            </span>
            <span aria-hidden className="block h-1.5 overflow-hidden rounded-full bg-surface-muted">
              <span
                className="block h-full rounded-full"
                style={{
                  width: `${Math.min(100, Math.max(0, item.percent))}%`,
                  backgroundColor: chartColor(index),
                }}
              />
            </span>
            {item.detail === undefined ? null : (
              <span className="block text-xs text-fg-muted">{item.detail}</span>
            )}
          </>
        );
        return (
          <li key={item.key}>
            {item.onPress === undefined ? (
              <div className="flex flex-col gap-1">{body}</div>
            ) : (
              <button
                type="button"
                onClick={item.onPress}
                className="flex w-full flex-col gap-1 rounded-sm text-left hover:bg-surface-muted"
              >
                {body}
              </button>
            )}
          </li>
        );
      })}
    </ul>
  );
}
