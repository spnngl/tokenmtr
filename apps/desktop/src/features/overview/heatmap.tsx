import type { ReactNode } from "react";
import { formatTokens } from "../../shared/format";
import type { Heatmap } from "../../shared/schemas";

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;
const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

export interface WeekdaySummary {
  readonly weekday: (typeof WEEKDAYS)[number];
  readonly events: number;
  readonly tokens: number;
  readonly apiValueMicros: number;
  /** Local hour with the most tokens; `null` when the weekday has no activity. */
  readonly busiestHour: number | null;
}

/** Per-weekday totals: the table alternative of the heatmap. */
export function summarizeWeekdays(heatmap: Heatmap): WeekdaySummary[] {
  return WEEKDAYS.map((weekday, index) => {
    const cells = heatmap.cells.filter((cell) => cell.weekday === index + 1);
    const busiest = cells.reduce<(typeof cells)[number] | null>(
      (best, cell) => (cell.total_tokens > (best?.total_tokens ?? 0) ? cell : best),
      null,
    );
    return {
      weekday,
      events: cells.reduce((sum, cell) => sum + cell.events, 0),
      tokens: cells.reduce((sum, cell) => sum + cell.total_tokens, 0),
      apiValueMicros: cells.reduce((sum, cell) => sum + cell.api_value_micros, 0),
      busiestHour: busiest?.hour ?? null,
    };
  });
}

/** Hour x weekday activity as a CSS grid; colour intensity is tokens relative to the busiest cell. */
export function HeatmapGrid({ heatmap }: { readonly heatmap: Heatmap }): ReactNode {
  const max = Math.max(1, ...heatmap.cells.map((cell) => cell.total_tokens));
  const tokensAt = (weekday: number, hour: number): number =>
    heatmap.cells.find((cell) => cell.weekday === weekday && cell.hour === hour)?.total_tokens ?? 0;
  return (
    <div className="min-w-0 overflow-x-auto">
      <div
        className="grid min-w-[480px] gap-px text-[10px] text-fg-muted"
        style={{ gridTemplateColumns: "2.5rem repeat(24, minmax(0, 1fr))" }}
      >
        <span />
        {HOURS.map((hour) => (
          <span key={hour} className="text-center">
            {hour % 6 === 0 ? hour : ""}
          </span>
        ))}
        {WEEKDAYS.map((weekday, index) => (
          <HeatRow
            key={weekday}
            label={weekday}
            weekday={index + 1}
            max={max}
            tokensAt={tokensAt}
          />
        ))}
      </div>
    </div>
  );
}

function HeatRow(props: {
  readonly label: string;
  readonly weekday: number;
  readonly max: number;
  readonly tokensAt: (weekday: number, hour: number) => number;
}): ReactNode {
  return (
    <>
      <span className="flex items-center text-xs">{props.label}</span>
      {HOURS.map((hour) => {
        const tokens = props.tokensAt(props.weekday, hour);
        return (
          <span
            key={hour}
            title={`${props.label} ${String(hour).padStart(2, "0")}:00 — ${formatTokens(tokens)} tokens`}
            className="aspect-square rounded-xs bg-surface-muted"
            style={
              tokens === 0
                ? undefined
                : {
                    backgroundColor: `color-mix(in oklab, var(--chart-1) ${Math.round(15 + (tokens / props.max) * 85)}%, transparent)`,
                  }
            }
          />
        );
      })}
    </>
  );
}
