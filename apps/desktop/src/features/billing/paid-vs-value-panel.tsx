import { ChevronLeft, ChevronRight } from "lucide-react";
import { type ReactNode, useState } from "react";
import {
  Area,
  Bar,
  CartesianGrid,
  ComposedChart,
  Legend,
  Line,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { useFilters } from "../../app/use-filters";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { AXIS, chartColor, compactMoney } from "../../shared/charts";
import {
  type Currency,
  formatCurrencyMicros,
  formatDay,
  formatRoiMultiple,
  PLACEHOLDER,
} from "../../shared/format";
import { useBillingSeries } from "../../shared/queries";
import { RemoteView } from "../../shared/remote-view";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { BillingPoint, BillingSeries, Granularity } from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
import { ChartFrame, DataTable, Section } from "../../shared/ui";
import { useChartAnimation } from "../../shared/use-chart-animation";
import { toRemote } from "../../shared/use-remote";
import {
  choiceOf,
  defaultGranularity,
  GRANULARITY_LABELS,
  granularityOptions,
  RANGE_CHOICES,
  type RangeChoice,
  rangeDays,
  rangeOf,
  shiftDate,
} from "./value-range";

/** One plotted value per bucket; the table alternative gets one column per series. */
interface ChartSeries {
  readonly key: string;
  readonly name: string;
  readonly color: string;
  readonly kind: "bar" | "line" | "area";
  /** Bars (or areas) sharing a stack id pile up. */
  readonly stack?: string;
  readonly value: (point: BillingPoint) => number | null;
}

type ChartRow = Record<string, string | number | null>;

const BUCKET_HEADER: Readonly<Record<Granularity, string>> = {
  hourly: "Hour",
  daily: "Day",
  monthly: "Month",
};

/** A bucket on the configured zone's clock: "14:00", "2026-06-15" or "2026-06". */
function bucketLabel(local: string, granularity: Granularity): string {
  switch (granularity) {
    case "hourly":
      return local.slice(11, 16);
    case "daily":
      return local.slice(0, 10);
    case "monthly":
      return local.slice(0, 7);
    default:
      return granularity satisfies never;
  }
}

interface FigureProps {
  readonly title: string;
  readonly description: string;
  readonly series: BillingSeries;
  readonly lines: readonly ChartSeries[];
  /** Values in the table and tooltip. */
  readonly format: (value: number) => string;
  /** Axis ticks. */
  readonly tick: (value: number) => string;
  /** Draws the zero line (savings and ROI can cross it). */
  readonly zeroLine?: boolean;
  readonly height?: number;
}

/** A chart over the buckets with its table alternative. */
function BucketFigure({
  title,
  description,
  series,
  lines,
  format,
  tick,
  zeroLine = false,
  height = 240,
}: FigureProps): ReactNode {
  const animate = useChartAnimation();
  const rows = series.points.map(
    (point): ChartRow => ({
      label: bucketLabel(point.local, series.granularity),
      ...Object.fromEntries(lines.map((line) => [line.key, line.value(point)])),
    }),
  );
  const show = (value: number | null): string => (value === null ? PLACEHOLDER : format(value));
  return (
    <ChartFrame
      interactive
      title={title}
      description={description}
      points={rows.length}
      table={
        <DataTable
          caption={title}
          rows={series.points}
          rowKey={(point) => point.start}
          columns={[
            {
              header: BUCKET_HEADER[series.granularity],
              cell: (point) => bucketLabel(point.local, series.granularity),
              sortValue: (point) => point.start,
            },
            ...lines.map((line) => ({
              header: line.name,
              numeric: true,
              cell: (point: BillingPoint) => show(line.value(point)),
              sortValue: (point: BillingPoint) => line.value(point) ?? Number.NEGATIVE_INFINITY,
            })),
          ]}
        />
      }
    >
      <ResponsiveChart height={height}>
        {(size) => (
          <ComposedChart
            width={size.width}
            height={size.height}
            data={rows}
            title={title}
            stackOffset="sign"
            margin={{ top: 8, right: 8, bottom: 4, left: 0 }}
          >
            <CartesianGrid vertical={false} stroke="var(--border)" />
            <XAxis dataKey="label" {...AXIS} minTickGap={16} />
            <YAxis {...AXIS} width={56} tickFormatter={tick} />
            <Tooltip
              formatter={(value: unknown) =>
                typeof value === "number" ? format(value) : String(value)
              }
            />
            <Legend />
            {zeroLine ? <ReferenceLine y={0} stroke="var(--fg-muted)" /> : null}
            {lines.map((line) => {
              const stack = line.stack === undefined ? {} : { stackId: line.stack };
              switch (line.kind) {
                case "bar":
                  return (
                    <Bar
                      key={line.key}
                      dataKey={line.key}
                      name={line.name}
                      fill={line.color}
                      isAnimationActive={animate}
                      {...stack}
                    />
                  );
                case "line":
                  return (
                    <Line
                      key={line.key}
                      dataKey={line.key}
                      name={line.name}
                      stroke={line.color}
                      strokeWidth={2}
                      dot={false}
                      isAnimationActive={animate}
                    />
                  );
                case "area":
                  return (
                    <Area
                      key={line.key}
                      dataKey={line.key}
                      name={line.name}
                      stroke={line.color}
                      fill={line.color}
                      fillOpacity={0.25}
                      isAnimationActive={animate}
                      {...stack}
                    />
                  );
                default:
                  return line.kind satisfies never;
              }
            })}
          </ComposedChart>
        )}
      </ResponsiveChart>
    </ChartFrame>
  );
}

/** Agent ids of a split series, highest API value first. */
function agentsOf(series: BillingSeries): string[] {
  const totals = new Map<string, number>();
  for (const point of series.points) {
    for (const [agent, amounts] of Object.entries(point.agents)) {
      totals.set(agent, (totals.get(agent) ?? 0) + amounts.api_value_micros + amounts.paid_micros);
    }
  }
  return [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([agent]) => agent);
}

/** Paid and API value side by side, or stacked per agent when split. */
function perBucketLines(series: BillingSeries, split: boolean): ChartSeries[] {
  if (!split) {
    return [
      {
        key: "paid",
        name: "Paid",
        color: chartColor(0),
        kind: "bar",
        value: (p) => p.paid_micros,
      },
      {
        key: "api",
        name: "API value",
        color: chartColor(1),
        kind: "bar",
        value: (p) => p.api_value_micros,
      },
    ];
  }
  const agents = agentsOf(series);
  const paid = agents.map(
    (agent, index): ChartSeries => ({
      key: `paid:${agent}`,
      name: `${agent} paid`,
      color: chartColor(index),
      kind: "bar",
      stack: "paid",
      value: (p) => p.agents[agent]?.paid_micros ?? 0,
    }),
  );
  const unassigned: ChartSeries = {
    key: "paid:fees",
    name: "Unassigned fees",
    color: "var(--fg-muted)",
    kind: "bar",
    stack: "paid",
    value: (p) =>
      p.paid_micros -
      Object.values(p.agents).reduce((sum, amounts) => sum + amounts.paid_micros, 0),
  };
  const api = agents.map(
    (agent, index): ChartSeries => ({
      key: `api:${agent}`,
      name: `${agent} API value`,
      color: chartColor(index + agents.length),
      kind: "bar",
      stack: "api",
      value: (p) => p.agents[agent]?.api_value_micros ?? 0,
    }),
  );
  return [...paid, unassigned, ...api];
}

function isQuiet(series: BillingSeries): boolean {
  return series.points.every((point) => point.paid_micros === 0 && point.api_value_micros === 0);
}

/** The five charts of one answer. */
export function PaidVsValueCharts({
  series,
  split,
}: {
  readonly series: BillingSeries;
  readonly split: boolean;
}): ReactNode {
  const currency: Currency = series.currency;
  const money = (value: number): string => formatCurrencyMicros(value, currency);
  const moneyTick = (value: number): string => compactMoney(value, currency);
  const bucket = BUCKET_HEADER[series.granularity].toLowerCase();
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <BucketFigure
        title={`Paid and API value per ${bucket}`}
        description={
          split
            ? "Paid and API value per agent; subscription fees of idle days stay unassigned."
            : "What you paid against what the same usage would cost at API prices."
        }
        series={series}
        lines={perBucketLines(series, split)}
        format={money}
        tick={moneyTick}
      />
      <BucketFigure
        title="Cumulative paid and API value"
        description="Running totals over the range."
        series={series}
        lines={[
          {
            key: "paid",
            name: "Paid",
            color: chartColor(0),
            kind: "line",
            value: (p) => p.cumulative_paid_micros,
          },
          {
            key: "api",
            name: "API value",
            color: chartColor(1),
            kind: "line",
            value: (p) => p.cumulative_api_value_micros,
          },
        ]}
        format={money}
        tick={moneyTick}
      />
      <BucketFigure
        title="What you paid"
        description="Paid split into subscription fees, Claude extra usage and pay-as-you-go API cost."
        series={series}
        lines={[
          {
            key: "fee",
            name: "Subscription fee",
            color: chartColor(0),
            kind: "bar",
            stack: "paid",
            value: (p) => p.subscription_fee_micros,
          },
          {
            key: "extra",
            name: "Extra usage",
            color: chartColor(3),
            kind: "bar",
            stack: "paid",
            value: (p) => p.extra_usage_micros,
          },
          {
            key: "payg",
            name: "Pay-as-you-go",
            color: chartColor(2),
            kind: "bar",
            stack: "paid",
            value: (p) => p.pay_as_you_go_micros,
          },
        ]}
        format={money}
        tick={moneyTick}
      />
      <BucketFigure
        title="Savings"
        description="API value minus paid; below zero you paid more than the usage was worth at API prices."
        series={series}
        lines={[
          {
            key: "savings",
            name: "Savings",
            color: chartColor(2),
            kind: "area",
            value: (p) => p.savings_micros,
          },
        ]}
        format={money}
        tick={moneyTick}
        zeroLine
      />
      <BucketFigure
        title="ROI"
        description="API value over paid per bucket; gaps where nothing was paid."
        series={series}
        lines={[
          {
            key: "roi",
            name: "ROI",
            color: chartColor(4),
            kind: "line",
            value: (p) => p.roi,
          },
        ]}
        format={(value) => formatRoiMultiple(value)}
        tick={(value) => `${value}×`}
        height={160}
      />
    </div>
  );
}

const SPLIT_OPTIONS = [
  { value: "total", label: "Total" },
  { value: "agent", label: "By agent" },
] as const;

/**
 * Paid against API value over a range of its own (today with day steps, 30 or 90 days, a year,
 * or the global filter range), bucketed by hour, day or month.
 */
export function PaidVsValuePanel(): ReactNode {
  const { filters } = useFilters();
  const { timeZone, now } = useAppConfig();
  const today = formatDay(now(), timeZone);
  const [picked, setPicked] = useState<RangeChoice | null>(null);
  const [dayOffset, setDayOffset] = useState(0);
  const [pickedGranularity, setPickedGranularity] = useState<Granularity | null>(null);
  const [split, setSplit] = useState(false);

  const choice = picked ?? choiceOf(filters.range);
  const range = rangeOf(choice, dayOffset, filters.range, today);
  const options = granularityOptions(rangeDays(range));
  const granularity =
    pickedGranularity !== null && options.includes(pickedGranularity)
      ? pickedGranularity
      : defaultGranularity(rangeDays(range));
  const query = useBillingSeries({ ...filters, range }, granularity, split);

  const choose = (next: RangeChoice): void => {
    setPicked(next);
    setDayOffset(0);
    setPickedGranularity(null);
  };

  return (
    <Section
      title="Paid against API value"
      description="Paid is subscription fees, Claude extra usage and pay-as-you-go cost; savings is API value minus paid, ROI is API value over paid."
    >
      <div className="flex flex-wrap items-center gap-2">
        <div className="max-w-full overflow-x-auto">
          <SegmentedControl
            label="Range"
            options={RANGE_CHOICES}
            value={choice}
            onChange={choose}
          />
        </div>
        {choice === "today" ? (
          <div className="flex items-center gap-1">
            <Button
              variant="quiet"
              aria-label="Previous day"
              onPress={() => setDayOffset((offset) => offset - 1)}
            >
              <ChevronLeft className="size-4" aria-hidden />
            </Button>
            <span className="text-sm tabular-nums">{shiftDate(today, dayOffset)}</span>
            <Button
              variant="quiet"
              aria-label="Next day"
              isDisabled={dayOffset === 0}
              onPress={() => setDayOffset((offset) => offset + 1)}
            >
              <ChevronRight className="size-4" aria-hidden />
            </Button>
          </div>
        ) : null}
        <SegmentedControl
          label="Granularity"
          options={options.map((value) => ({ value, label: GRANULARITY_LABELS[value] }))}
          value={granularity}
          onChange={setPickedGranularity}
        />
        <SegmentedControl
          label="Split"
          options={SPLIT_OPTIONS}
          value={split ? "agent" : "total"}
          onChange={(value) => setSplit(value === "agent")}
        />
      </div>
      <RemoteView
        remote={toRemote(query)}
        size="section"
        emptyWhen={(series) =>
          isQuiet(series)
            ? { title: "Nothing paid or used", hint: "No fee or usage in this range." }
            : null
        }
      >
        {(series) => <PaidVsValueCharts series={series} split={split} />}
      </RemoteView>
    </Section>
  );
}
