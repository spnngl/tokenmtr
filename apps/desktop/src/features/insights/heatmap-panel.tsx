import { type ReactNode, useState } from "react";
import type { Currency } from "../../shared/format";
import { formatCount, formatCurrencyMicros, formatTokens } from "../../shared/format";
import type { Heatmap } from "../../shared/schemas/usage";
import { SegmentedControl } from "../../shared/segmented-control";
import { Section } from "../../shared/ui";
import { type HeatMetric, heatGrid, heatIntensity, WEEKDAYS } from "./model";

const METRICS = [
  { value: "events", label: "Events" },
  { value: "total_tokens", label: "Tokens" },
  { value: "api_value_micros", label: "API value" },
] as const satisfies readonly { value: HeatMetric; label: string }[];

const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

function formatMetric(value: number, metric: HeatMetric, currency: Currency): string {
  switch (metric) {
    case "events":
      return formatCount(value);
    case "total_tokens":
      return formatTokens(value);
    case "api_value_micros":
      return formatCurrencyMicros(value, currency);
    default:
      return metric satisfies never;
  }
}

/** Fill of a cell: the accent over the surface, in proportion to the cell's share of the peak. */
function cellStyle(intensity: number): { backgroundColor: string } {
  return {
    backgroundColor: `color-mix(in srgb, var(--accent) ${Math.round(intensity * 100)}%, var(--surface-muted))`,
  };
}

/**
 * Weekday by hour activity (real traffic only, local time zone). A real table, so a screen reader
 * gets every value; intensity is a visual aid and every cell also carries its value as text.
 */
export function HeatmapPanel({
  heatmap,
  timeZone,
}: {
  readonly heatmap: Heatmap;
  readonly timeZone: string;
}): ReactNode {
  const [metric, setMetric] = useState<HeatMetric>("events");
  const grid = heatGrid(heatmap.cells, metric);
  return (
    <Section
      title="Activity by hour"
      description={`When you work, by weekday and hour in ${timeZone}. Darker means more.`}
      actions={
        <SegmentedControl
          label="Heatmap metric"
          options={METRICS}
          value={metric}
          onChange={setMetric}
        />
      }
    >
      <section
        aria-label="Activity by weekday and hour"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll region must be focusable (WCAG 2.1.1).
        tabIndex={0}
        className="overflow-x-auto rounded-md border border-border"
      >
        <table className="border-collapse text-xs" data-points={heatmap.cells.length}>
          <caption className="sr-only">Activity by weekday and hour</caption>
          <thead>
            <tr>
              <td className="p-1" />
              {HOURS.map((hour) => (
                <th key={hour} scope="col" className="min-w-7 p-1 font-normal text-fg-muted">
                  {hour}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {grid.rows.map((values, weekdayIndex) => (
              <tr key={WEEKDAYS[weekdayIndex]}>
                <th scope="row" className="p-1 pr-2 text-left font-normal text-fg-muted">
                  {WEEKDAYS[weekdayIndex]}
                </th>
                {HOURS.map((hour) => {
                  const value = values[hour] ?? 0;
                  const text = formatMetric(value, metric, heatmap.currency);
                  return (
                    <td
                      key={hour}
                      title={`${WEEKDAYS[weekdayIndex]} ${hour}:00 - ${text}`}
                      style={cellStyle(heatIntensity(value, grid.max))}
                      className="h-7 min-w-7 border border-surface text-center"
                    >
                      <span className="sr-only">{text}</span>
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </section>
    </Section>
  );
}
