import { type ReactNode, useState } from "react";
import type { Currency } from "../../shared/format";
import { formatCount, formatCurrencyMicros, formatTokens } from "../../shared/format";
import type { Heatmap } from "../../shared/schemas/usage";
import { SegmentedControl } from "../../shared/segmented-control";
import { Section } from "../../shared/ui";
import { WeekHourTable } from "../../shared/week-hour-table";
import { type HeatMetric, heatGrid } from "./model";

const METRICS = [
  { value: "events", label: "Events" },
  { value: "total_tokens", label: "Tokens" },
  { value: "api_value_micros", label: "API value" },
] as const satisfies readonly { value: HeatMetric; label: string }[];

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

/** Weekday by hour activity (real traffic only, local time zone). */
export function HeatmapPanel({
  heatmap,
  timeZone,
}: {
  readonly heatmap: Heatmap;
  readonly timeZone: string;
}): ReactNode {
  const [metric, setMetric] = useState<HeatMetric>("events");
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
      <WeekHourTable
        label="Activity by weekday and hour"
        grid={heatGrid(heatmap.cells, metric)}
        format={(value) => formatMetric(value, metric, heatmap.currency)}
      />
    </Section>
  );
}
