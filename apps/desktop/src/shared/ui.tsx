import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  CheckCircle2,
  Info,
  XCircle,
} from "lucide-react";
import { type ReactNode, useId, useState } from "react";
import { type Currency, formatCurrencyMicros, LOCALE } from "./format";
import { type NavPath, navItem } from "./nav";

/**
 * The primitives every screen is built from: page and section headings, cards, KPI tiles, chart
 * frames, tables, money figures and status badges.
 */

type PageHeaderProps =
  | { readonly to: NavPath }
  | { readonly title: string; readonly summary: string };

/** Screen title and one line of purpose; `to` reads both from the nav, so the text lives once. */
export function PageHeader(props: PageHeaderProps): ReactNode {
  const { title, summary } =
    "to" in props ? { title: navItem(props.to).label, summary: navItem(props.to).summary } : props;
  return (
    <header className="flex flex-col gap-1">
      <h1 className="text-xl font-semibold">{title}</h1>
      <p className="text-sm text-fg-muted">{summary}</p>
    </header>
  );
}

interface SectionProps {
  readonly title: string;
  readonly description?: string;
  readonly children: ReactNode;
  /** Controls next to the title, e.g. a segmented control. */
  readonly actions?: ReactNode;
}

/** A titled block of a screen. */
export function Section({ title, description, children, actions }: SectionProps): ReactNode {
  return (
    <section aria-label={title} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <h2 className="text-base font-semibold">{title}</h2>
          {description === undefined ? null : (
            <p className="text-sm text-fg-muted">{description}</p>
          )}
        </div>
        {actions}
      </div>
      {children}
    </section>
  );
}

/** A titled surface. The heading names the region for assistive tech. */
export function Card(props: {
  readonly title: string;
  readonly actions?: ReactNode;
  readonly children: ReactNode;
}): ReactNode {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      className="flex min-w-0 flex-col gap-3 rounded-md border border-border bg-surface-raised p-4"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id={id} className="text-base font-semibold">
          {props.title}
        </h2>
        {props.actions}
      </div>
      {props.children}
    </section>
  );
}

/** One headline figure. `detail` carries the qualifier (e.g. "estimated"). Wrap in `KpiGrid`. */
export function Kpi(props: {
  readonly label: string;
  readonly value: ReactNode;
  readonly detail?: ReactNode;
}): ReactNode {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-md border border-border bg-surface-raised p-3">
      <dt className="text-sm text-fg-muted">{props.label}</dt>
      <dd className="text-xl font-semibold tabular-nums [overflow-wrap:anywhere]">{props.value}</dd>
      {props.detail === undefined ? null : (
        <dd className="text-xs text-fg-muted">{props.detail}</dd>
      )}
    </div>
  );
}

/** A grid of `Kpi`s: a single description list so screen readers announce it as one. */
export function KpiGrid({ children }: { readonly children: ReactNode }): ReactNode {
  return <dl className="grid grid-cols-2 gap-3 md:grid-cols-4">{children}</dl>;
}

interface ChartFrameProps {
  /** Names the chart; also the accessible name of its figure. */
  readonly title: string;
  /** What the chart shows, in a sentence: the text alternative of the picture. */
  readonly description: string;
  /** Number of plotted points, exposed for tests and the D47 budget. */
  readonly points: number;
  /** The same data as a table (`DataTable`), shown on demand. */
  readonly table: ReactNode;
  /** Controls next to the title, e.g. a segmented control. */
  readonly actions?: ReactNode;
  readonly children: ReactNode;
}

/**
 * Accessible chart wrapper: a figure with a caption, a description, and the data as a table
 * behind a disclosure (PLAN §7: every chart has one). The picture itself is hidden from assistive
 * tech so the table, which carries every value, is the only thing it reads.
 */
export function ChartFrame({
  title,
  description,
  points,
  table,
  actions,
  children,
}: ChartFrameProps): ReactNode {
  const descriptionId = useId();
  return (
    <figure
      aria-label={title}
      aria-describedby={descriptionId}
      data-points={points}
      className="flex min-w-0 flex-col gap-2 rounded-md border border-border bg-surface-raised p-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <figcaption className="text-sm font-medium">{title}</figcaption>
        {actions}
      </div>
      <p id={descriptionId} className="text-xs text-fg-muted">
        {description}
      </p>
      <div aria-hidden className="min-w-0">
        {children}
      </div>
      <details className="text-sm">
        <summary className="cursor-pointer rounded-md py-1 text-fg-muted focus-visible:outline-2 focus-visible:outline-ring">
          View as table
        </summary>
        <div className="pt-2">{table}</div>
      </details>
    </figure>
  );
}

/** A money figure; `approximate` adds a visible and a spoken qualifier. */
export function Amount(props: {
  readonly micros: number;
  readonly currency: Currency;
  /** The row's `approximate` flag (estimates, unpriced events, daily aggregates, old FX rate). */
  readonly approximate?: boolean;
}): ReactNode {
  const text = formatCurrencyMicros(props.micros, props.currency);
  if (props.approximate !== true) {
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

export interface Column<Row> {
  /** Unique in the table: names the column and its sort state. */
  readonly header: string;
  readonly cell: (row: Row) => ReactNode;
  /** Present when the column can be sorted. */
  readonly sortValue?: (row: Row) => number | string;
  /** Numbers and money align to the end. */
  readonly numeric?: boolean;
}

type SortState = { readonly header: string; readonly direction: "asc" | "desc" } | null;

function compare(a: number | string, b: number | string): number {
  if (typeof a === "number" && typeof b === "number") {
    return a - b;
  }
  return String(a).localeCompare(String(b), LOCALE);
}

/**
 * A semantic table; sortable columns toggle through header buttons (`aria-sort`). It is the text
 * alternative of every chart. Wide tables scroll inside a focusable region, so keyboard users can
 * reach every column.
 */
export function DataTable<Row>(props: {
  readonly caption: string;
  readonly columns: readonly Column<Row>[];
  readonly rows: readonly Row[];
  /** Unique among rows; `index` is for data with no natural key. */
  readonly rowKey: (row: Row, index: number) => string;
}): ReactNode {
  const [sort, setSort] = useState<SortState>(null);
  const sortColumn = props.columns.find((column) => column.header === sort?.header);
  const sortValue = sortColumn?.sortValue;
  const rows =
    sort === null || sortValue === undefined
      ? props.rows
      : [...props.rows].sort(
          (a, b) => compare(sortValue(a), sortValue(b)) * (sort.direction === "asc" ? 1 : -1),
        );

  const toggle = (header: string): void =>
    setSort((current) => {
      if (current?.header !== header) {
        return { header, direction: "desc" };
      }
      return current.direction === "desc" ? { header, direction: "asc" } : null;
    });

  return (
    <section
      aria-label={props.caption}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll region must be focusable (WCAG 2.1.1).
      tabIndex={0}
      className="min-w-0 overflow-x-auto rounded-md border border-border"
    >
      <table className="w-full min-w-max border-collapse text-sm">
        <caption className="sr-only">{props.caption}</caption>
        <thead className="bg-surface-muted text-left text-xs text-fg-muted">
          <tr>
            {props.columns.map((column) => {
              const active = sort?.header === column.header ? sort.direction : null;
              return (
                <th
                  key={column.header}
                  scope="col"
                  aria-sort={
                    active === null ? undefined : active === "asc" ? "ascending" : "descending"
                  }
                  className={`px-3 py-2 font-medium ${column.numeric === true ? "text-right" : ""}`}
                >
                  {column.sortValue === undefined ? (
                    column.header
                  ) : (
                    <button
                      type="button"
                      onClick={() => toggle(column.header)}
                      className="inline-flex items-center gap-1 rounded-sm font-medium"
                    >
                      {column.header}
                      {active === null ? (
                        <ArrowUpDown className="size-3" aria-hidden />
                      ) : active === "asc" ? (
                        <ArrowUp className="size-3" aria-hidden />
                      ) : (
                        <ArrowDown className="size-3" aria-hidden />
                      )}
                    </button>
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={props.rowKey(row, index)} className="border-t border-border">
              {props.columns.map((column) => (
                <td
                  key={column.header}
                  className={`px-3 py-2 tabular-nums ${column.numeric === true ? "text-right" : ""}`}
                >
                  {column.cell(row)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

export type Tone = "neutral" | "success" | "warning" | "danger";

function icon(tone: Tone): ReactNode {
  switch (tone) {
    case "neutral":
      return <Info aria-hidden className="size-3.5" />;
    case "success":
      return <CheckCircle2 aria-hidden className="size-3.5" />;
    case "warning":
      return <AlertTriangle aria-hidden className="size-3.5" />;
    case "danger":
      return <XCircle aria-hidden className="size-3.5" />;
    default:
      return tone satisfies never;
  }
}

function toneClass(tone: Tone): string {
  switch (tone) {
    case "neutral":
      return "text-fg-muted";
    case "success":
      return "text-success";
    case "warning":
      return "text-warning";
    case "danger":
      return "text-danger";
    default:
      return tone satisfies never;
  }
}

/** A short status label. The icon and the text carry the meaning, the colour only reinforces. */
export function Badge({
  tone,
  children,
}: {
  readonly tone: Tone;
  readonly children: ReactNode;
}): ReactNode {
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-md border border-border px-1.5 py-0.5 text-xs font-medium whitespace-nowrap ${toneClass(tone)}`}
    >
      {icon(tone)}
      {children}
    </span>
  );
}
