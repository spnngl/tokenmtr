import { CircleAlert, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { Label, Meter } from "react-aria-components";
import { formatPercent } from "./format";

export type MeterLevel = "ok" | "warning" | "danger";

/** Amber at >= 70%, red at >= 90% (PLAN §7). */
function meterLevel(usedFraction: number): MeterLevel {
  if (usedFraction >= 0.9) {
    return "danger";
  }
  if (usedFraction >= 0.7) {
    return "warning";
  }
  return "ok";
}

const FILL: Record<MeterLevel, string> = {
  ok: "bg-accent",
  warning: "bg-warning",
  danger: "bg-danger",
};

interface UsageMeterProps {
  readonly label: string;
  /** 0..1; values above 1 fill the bar and still print their true percentage. */
  readonly usedFraction: number;
  /** A line under the bar, e.g. "resets in 2h 15m". */
  readonly detail?: ReactNode;
}

/** A labelled meter. Warning and danger levels add an icon, so colour is never the only cue. */
export function UsageMeter({ label, usedFraction, detail }: UsageMeterProps): ReactNode {
  const level = meterLevel(usedFraction);
  return (
    <Meter value={Math.min(usedFraction, 1) * 100} className="flex flex-col gap-1">
      {({ percentage }) => (
        <>
          <div className="flex items-center justify-between gap-2 text-sm">
            <Label className="min-w-0 truncate font-medium">{label}</Label>
            <span className="flex items-center gap-1 tabular-nums">
              {level === "warning" ? (
                <TriangleAlert className="size-4 text-warning" aria-hidden />
              ) : null}
              {level === "danger" ? (
                <CircleAlert className="size-4 text-danger" aria-hidden />
              ) : null}
              {formatPercent(usedFraction)}
            </span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-surface-muted">
            <div
              className={`h-full rounded-full ${FILL[level]}`}
              style={{ width: `${percentage}%` }}
            />
          </div>
          {detail === undefined ? null : <p className="text-xs text-fg-muted">{detail}</p>}
        </>
      )}
    </Meter>
  );
}
