import { type ReactNode, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { formatCount, formatTokens, LOCALE } from "../../shared/format";
import { useActivityCalendar } from "../../shared/queries";
import { CardRemote } from "../../shared/remote-view";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { ActivityCalendar, ActivityDay } from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
import { ChartFrame, DataTable } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";

type CalendarMetric = "tokens" | "sessions";

const METRICS: readonly { readonly value: CalendarMetric; readonly label: string }[] = [
  { value: "tokens", label: "Tokens" },
  { value: "sessions", label: "Sessions" },
];

/** Square side and gap, px: the grid's width decides how many weeks show, never the squares'. */
const CELL = 11;
const GAP = 3;
const PITCH = CELL + GAP;
/** Room for the weekday labels on the left and the month labels on top. */
const LABEL_WIDTH = 28;
const HEADER_HEIGHT = 16;
/** A narrow window still gets a full year, scrolled to today. */
const MIN_WEEKS = 53;
/** Non-zero fill levels; level 0 is an idle day. */
const LEVELS = 4;
const LEGEND = Array.from({ length: LEVELS + 1 }, (_, level) => level);

const DAY_FORMAT = new Intl.DateTimeFormat(LOCALE, {
  weekday: "short",
  month: "short",
  day: "numeric",
  year: "numeric",
  timeZone: "UTC",
});
const MONTH_FORMAT = new Intl.DateTimeFormat(LOCALE, { month: "short", timeZone: "UTC" });
const MONTH_YEAR_FORMAT = new Intl.DateTimeFormat(LOCALE, {
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});
const WEEKDAY_FORMAT = new Intl.DateTimeFormat(LOCALE, { weekday: "short", timeZone: "UTC" });

/** A civil `YYYY-MM-DD` day as the UTC midnight that formats back to the same day. */
function utc(day: string): Date {
  return new Date(`${day}T00:00:00Z`);
}

function measure(day: ActivityDay, metric: CalendarMetric): number {
  return metric === "tokens" ? day.total_tokens : day.sessions;
}

function formatValue(value: number, metric: CalendarMetric): string {
  return metric === "tokens" ? `${formatTokens(value)} tokens` : `${formatCount(value)} sessions`;
}

/** The days in week columns; the backend starts them on the configured first day of the week. */
export function weeksOf(days: readonly ActivityDay[]): ActivityDay[][] {
  const weeks: ActivityDay[][] = [];
  for (let i = 0; i < days.length; i += 7) {
    weeks.push(days.slice(i, i + 7));
  }
  return weeks;
}

/** Weeks that fit `width` px, at least a year (it then scrolls), at most every week there is. */
export function weeksThatFit(width: number, total: number): number {
  const fit = Math.floor((width - LABEL_WIDTH + GAP) / PITCH);
  return Math.min(total, Math.max(MIN_WEEKS, fit));
}

/**
 * Fill level 0-4 per value, GitHub style: idle is 0, the active days split in quartiles so a few
 * huge days do not wash the rest out.
 */
export function levelsOf(values: readonly number[]): (value: number) => number {
  const active = values.filter((value) => value > 0).toSorted((a, b) => a - b);
  const thresholds = Array.from(
    { length: LEVELS - 1 },
    (_, i) => active[Math.floor(((i + 1) * active.length) / LEVELS)] ?? 0,
  );
  return (value) =>
    value <= 0 ? 0 : 1 + thresholds.filter((threshold) => value > threshold).length;
}

function fill(level: number): string {
  return level === 0
    ? "var(--surface-muted)"
    : `color-mix(in srgb, var(--accent) ${Math.round((level / LEVELS) * 100)}%, var(--surface-muted))`;
}

interface MonthRow {
  readonly month: string;
  readonly tokens: number;
  readonly sessions: number;
  readonly activeDays: number;
}

/** Per-month totals: the table alternative of the grid. */
export function monthsOf(days: readonly ActivityDay[]): MonthRow[] {
  const months = new Map<string, MonthRow>();
  for (const day of days) {
    const month = day.day.slice(0, 7);
    const row = months.get(month) ?? { month, tokens: 0, sessions: 0, activeDays: 0 };
    months.set(month, {
      month,
      tokens: row.tokens + day.total_tokens,
      sessions: row.sessions + day.sessions,
      activeDays: row.activeDays + (day.total_tokens > 0 || day.sessions > 0 ? 1 : 0),
    });
  }
  return [...months.values()];
}

/** GitHub-style daily activity over the last two years, tokens or started sessions. */
export function ActivityCalendarCard(): ReactNode {
  const { filters } = useFilters();
  const [metric, setMetric] = useState<CalendarMetric>("tokens");
  return (
    <CardRemote title="Daily activity" remote={toRemote(useActivityCalendar(filters))}>
      {(calendar) => (
        <CalendarFrame calendar={calendar} metric={metric} onMetricChange={setMetric} />
      )}
    </CardRemote>
  );
}

function CalendarFrame(props: {
  readonly calendar: ActivityCalendar;
  readonly metric: CalendarMetric;
  readonly onMetricChange: (metric: CalendarMetric) => void;
}): ReactNode {
  const { calendar, metric } = props;
  const total = calendar.days.reduce((sum, day) => sum + measure(day, metric), 0);
  const active = calendar.days.filter((day) => measure(day, metric) > 0).length;
  return (
    <ChartFrame
      title="Daily activity"
      description={`${formatValue(total, metric)} over the last two years, on ${formatCount(active)} active days. The grid shows as many recent weeks as fit.`}
      points={calendar.days.length}
      actions={
        <SegmentedControl
          label="Activity measure"
          options={METRICS}
          value={metric}
          onChange={props.onMetricChange}
        />
      }
      table={
        <DataTable
          caption="Activity by month"
          rows={monthsOf(calendar.days)}
          rowKey={(row) => row.month}
          columns={[
            {
              header: "Month",
              cell: (row) => MONTH_YEAR_FORMAT.format(utc(`${row.month}-01`)),
              sortValue: (row) => row.month,
            },
            {
              header: "Tokens",
              numeric: true,
              cell: (row) => formatTokens(row.tokens),
              sortValue: (row) => row.tokens,
            },
            {
              header: "Sessions",
              numeric: true,
              cell: (row) => formatCount(row.sessions),
              sortValue: (row) => row.sessions,
            },
            {
              header: "Active days",
              numeric: true,
              cell: (row) => formatCount(row.activeDays),
              sortValue: (row) => row.activeDays,
            },
          ]}
        />
      }
    >
      <ResponsiveChart height={HEADER_HEIGHT + 7 * PITCH}>
        {({ width }) => <CalendarGrid days={calendar.days} metric={metric} width={width} />}
      </ResponsiveChart>
    </ChartFrame>
  );
}

function CalendarGrid(props: {
  readonly days: readonly ActivityDay[];
  readonly metric: CalendarMetric;
  readonly width: number;
}): ReactNode {
  const { metric } = props;
  const all = weeksOf(props.days);
  const weeks = all.slice(all.length - weeksThatFit(props.width, all.length));
  const levelOf = levelsOf(weeks.flat().map((day) => measure(day, metric)));
  const height = HEADER_HEIGHT + 7 * PITCH - GAP;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex text-[10px]">
        {/* Outside the scroller, so the weekdays stay in view while scrolling. */}
        <svg width={LABEL_WIDTH} height={height} aria-hidden="true" className="shrink-0">
          {weeks[0]?.map((day, row) =>
            row % 2 === 1 ? (
              <text
                key={day.day}
                x={0}
                y={HEADER_HEIGHT + row * PITCH + CELL - 1}
                className="fill-fg-muted"
              >
                {WEEKDAY_FORMAT.format(utc(day.day))}
              </text>
            ) : null,
          )}
        </svg>
        {/* A reversed row scrolls from its end: it opens on today, a narrow window scrolls back. */}
        <section
          aria-label="Daily activity grid"
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll region must be focusable (WCAG 2.1.1).
          tabIndex={0}
          className="flex min-w-0 flex-1 flex-row-reverse overflow-x-auto"
        >
          <svg
            width={weeks.length * PITCH - GAP}
            height={height}
            aria-hidden="true"
            className="shrink-0"
          >
            {monthLabels(weeks).map(({ column, label }) => (
              <text key={column} x={column * PITCH} y={HEADER_HEIGHT - 5} className="fill-fg-muted">
                {label}
              </text>
            ))}
            {weeks.map((week, column) =>
              week.map((day, row) => {
                const value = measure(day, metric);
                return (
                  <rect
                    key={day.day}
                    x={column * PITCH}
                    y={HEADER_HEIGHT + row * PITCH}
                    width={CELL}
                    height={CELL}
                    rx={2}
                    style={{ fill: fill(levelOf(value)) }}
                  >
                    <title>{`${DAY_FORMAT.format(utc(day.day))}: ${formatValue(value, metric)}`}</title>
                  </rect>
                );
              }),
            )}
          </svg>
        </section>
      </div>
      <Legend />
    </div>
  );
}

/** Columns a month label spans; closer labels, or one closer to the right edge, would overlap. */
const LABEL_COLUMNS = 3;

/** A label above each column holding a month's first day, unless it would overlap. */
function monthLabels(
  weeks: readonly (readonly ActivityDay[])[],
): { readonly column: number; readonly label: string }[] {
  const labels: { column: number; label: string }[] = [];
  weeks.forEach((week, column) => {
    const first = week.find((day) => day.day.endsWith("-01"));
    const previous = labels[labels.length - 1];
    if (
      first !== undefined &&
      column <= weeks.length - LABEL_COLUMNS &&
      (previous === undefined || column - previous.column >= LABEL_COLUMNS)
    ) {
      labels.push({ column, label: MONTH_FORMAT.format(utc(first.day)) });
    }
  });
  return labels;
}

function Legend(): ReactNode {
  return (
    <div className="flex items-center justify-end gap-1 text-xs text-fg-muted">
      <span className="pr-1">Less</span>
      {LEGEND.map((level) => (
        <span
          key={level}
          className="inline-block rounded-xs"
          style={{ width: CELL, height: CELL, backgroundColor: fill(level) }}
        />
      ))}
      <span className="pl-1">More</span>
    </div>
  );
}
