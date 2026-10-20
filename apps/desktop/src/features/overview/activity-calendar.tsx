import { type KeyboardEvent, type ReactNode, useId, useRef, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { formatCount, formatTokens, LOCALE } from "../../shared/format";
import { useActivityCalendar } from "../../shared/queries";
import { CardRemote } from "../../shared/remote-view";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { ActivityCalendar, ActivityDay } from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
import { ChartFrame, DataTable } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";

export type CalendarMetric = "tokens" | "sessions";

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

interface Streaks {
  /** Active days in a row up to today; an idle today does not break it yet. */
  readonly current: number;
  readonly longest: number;
  /** The first day with the highest value; none when every day is idle. */
  readonly busiest: ActivityDay | undefined;
}

export function streaksOf(days: readonly ActivityDay[], metric: CalendarMetric): Streaks {
  let run = 0;
  let longest = 0;
  let busiest: ActivityDay | undefined;
  for (const day of days) {
    const value = measure(day, metric);
    run = value > 0 ? run + 1 : 0;
    longest = Math.max(longest, run);
    if (value > 0 && (busiest === undefined || value > measure(busiest, metric))) {
      busiest = day;
    }
  }
  const last = days.at(-1);
  let current = 0;
  const end = last !== undefined && measure(last, metric) === 0 ? days.length - 1 : days.length;
  for (let i = end - 1; i >= 0; i -= 1) {
    const day = days[i];
    if (day === undefined || measure(day, metric) === 0) {
      break;
    }
    current += 1;
  }
  return { current, longest, busiest };
}

/**
 * GitHub-style daily activity over the last two years, tokens or started sessions. Nothing when
 * every day is idle. The caller owns `metric`, so the choice survives a remount.
 */
export function ActivityCalendarCard(props: {
  readonly metric: CalendarMetric;
  readonly onMetricChange: (metric: CalendarMetric) => void;
}): ReactNode {
  const { filters } = useFilters();
  return (
    <CardRemote title="Daily activity" remote={toRemote(useActivityCalendar(filters))}>
      {(calendar) =>
        calendar.days.some((day) => day.total_tokens > 0 || day.sessions > 0) ? (
          <CalendarFrame
            calendar={calendar}
            metric={props.metric}
            onMetricChange={props.onMetricChange}
          />
        ) : null
      }
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
  // Over every day, not the visible weeks: a day keeps its shade when the window resizes.
  const levelOf = levelsOf(calendar.days.map((day) => measure(day, metric)));
  return (
    <ChartFrame
      title="Daily activity"
      description={`${formatValue(total, metric)} over the last two years, on ${formatCount(active)} active days. The grid shows as many recent weeks as fit.`}
      // Plain rects, not Recharts: the ~400-point budget of `ChartFrame` does not apply.
      points={calendar.days.length}
      interactive
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
      <div className="flex flex-col gap-3">
        <StreakStats streaks={streaksOf(calendar.days, metric)} metric={metric} />
        <ResponsiveChart height={HEADER_HEIGHT + 7 * PITCH}>
          {({ width }) => (
            <CalendarGrid days={calendar.days} metric={metric} levelOf={levelOf} width={width} />
          )}
        </ResponsiveChart>
      </div>
    </ChartFrame>
  );
}

function formatDays(count: number): string {
  return `${formatCount(count)} ${count === 1 ? "day" : "days"}`;
}

function StreakStats(props: {
  readonly streaks: Streaks;
  readonly metric: CalendarMetric;
}): ReactNode {
  const { current, longest, busiest } = props.streaks;
  const stats = [
    { label: "Current streak", value: formatDays(current) },
    { label: "Longest streak", value: formatDays(longest) },
    {
      label: "Busiest day",
      value:
        busiest === undefined
          ? "None"
          : `${DAY_FORMAT.format(utc(busiest.day))} · ${formatValue(measure(busiest, props.metric), props.metric)}`,
    },
  ];
  return (
    <dl className="flex flex-wrap gap-x-6 gap-y-1 text-xs">
      {stats.map((stat) => (
        <div key={stat.label} className="flex gap-1.5">
          <dt className="text-fg-muted">{stat.label}</dt>
          <dd className="font-medium text-fg tabular-nums">{stat.value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Days an arrow key moves: a column is a week, a row a day. */
const ARROW_STEPS: Readonly<Record<string, number>> = {
  ArrowLeft: -7,
  ArrowRight: 7,
  ArrowUp: -1,
  ArrowDown: 1,
};

function CalendarGrid(props: {
  readonly days: readonly ActivityDay[];
  readonly metric: CalendarMetric;
  readonly levelOf: (value: number) => number;
  readonly width: number;
}): ReactNode {
  const { metric, levelOf } = props;
  const all = weeksOf(props.days);
  const weeks = all.slice(all.length - weeksThatFit(props.width, all.length));
  const visible = weeks.flat();
  const today = visible.at(-1);
  // The day under the pointer or the keyboard; the readout shows today without one. Only
  // keyboard moves are announced: a pointer sweep would read out every day it crosses.
  const [active, setActive] = useState<{
    readonly day: string;
    readonly by: "pointer" | "keyboard";
  } | null>(null);
  const shown = visible.find((day) => day.day === active?.day) ?? today;
  const scroller = useRef<HTMLElement>(null);
  const hintId = useId();
  const height = HEADER_HEIGHT + 7 * PITCH - GAP;

  const onKeyDown = (event: KeyboardEvent<HTMLElement>): void => {
    const from = visible.findIndex((day) => day.day === shown?.day);
    const step = ARROW_STEPS[event.key];
    const to =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? visible.length - 1
          : step === undefined
            ? null
            : from + step;
    if (to === null) {
      return;
    }
    event.preventDefault();
    const day = visible[to];
    if (day !== undefined) {
      setActive({ day: day.day, by: "keyboard" });
      event.currentTarget
        .querySelector(`[data-day="${day.day}"]`)
        ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    }
  };

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
          ref={scroller}
          aria-label="Daily activity grid"
          aria-describedby={hintId}
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll region must be focusable (WCAG 2.1.1).
          tabIndex={0}
          onKeyDown={onKeyDown}
          onFocus={() =>
            setActive((current) =>
              today === undefined ? current : (current ?? { day: today.day, by: "keyboard" }),
            )
          }
          onBlur={() => setActive(null)}
          className="flex min-w-0 flex-1 flex-row-reverse overflow-x-auto rounded-xs focus-visible:outline-2 focus-visible:outline-ring"
        >
          <svg
            width={weeks.length * PITCH - GAP}
            height={height}
            aria-hidden="true"
            className="shrink-0"
            onPointerLeave={() => {
              if (document.activeElement !== scroller.current) {
                setActive(null);
              }
            }}
          >
            {monthLabels(weeks).map(({ column, label }) => (
              <text key={column} x={column * PITCH} y={HEADER_HEIGHT - 5} className="fill-fg-muted">
                {label}
              </text>
            ))}
            {weeks.map((week, column) =>
              week.map((day, row) => (
                <rect
                  key={day.day}
                  data-day={day.day}
                  x={column * PITCH}
                  y={HEADER_HEIGHT + row * PITCH}
                  width={CELL}
                  height={CELL}
                  rx={2}
                  style={{
                    fill: fill(levelOf(measure(day, metric))),
                    stroke:
                      active !== null && day.day === shown?.day
                        ? "var(--fg)"
                        : day.day === today?.day
                          ? "var(--fg-muted)"
                          : undefined,
                  }}
                  onPointerEnter={() => setActive({ day: day.day, by: "pointer" })}
                />
              )),
            )}
          </svg>
        </section>
      </div>
      <p id={hintId} hidden>
        Arrow keys move between days, Home and End to the first and last.
      </p>
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-fg-muted">
        <p
          role="status"
          aria-live={active?.by === "keyboard" ? "polite" : "off"}
          className="tabular-nums"
        >
          {shown === undefined ? null : (
            <>
              {DAY_FORMAT.format(utc(shown.day))}
              {shown.day === today?.day ? " (today)" : ""}:{" "}
              <span className="font-medium text-fg">
                {formatValue(measure(shown, metric), metric)}
              </span>
            </>
          )}
        </p>
        <Legend />
      </div>
    </div>
  );
}

/** Columns a month label spans; closer labels, or one closer to the right edge, would overlap. */
const LABEL_COLUMNS = 3;

/**
 * A label above each column whose top day starts a month, as on GitHub, unless it would overlap.
 * Labels are placed from the right, so a month start wins over the partial month at the left edge.
 */
function monthLabels(
  weeks: readonly (readonly ActivityDay[])[],
): { readonly column: number; readonly label: string }[] {
  const labels: { column: number; label: string }[] = [];
  for (let column = weeks.length - LABEL_COLUMNS; column >= 0; column -= 1) {
    const top = weeks[column]?.[0];
    const month = top?.day.slice(0, 7);
    const next = labels[labels.length - 1];
    if (
      top !== undefined &&
      month !== weeks[column - 1]?.[0]?.day.slice(0, 7) &&
      (next === undefined || next.column - column >= LABEL_COLUMNS)
    ) {
      labels.push({ column, label: MONTH_FORMAT.format(utc(top.day)) });
    }
  }
  return labels.reverse();
}

function Legend(): ReactNode {
  return (
    <div className="flex items-center gap-1">
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
