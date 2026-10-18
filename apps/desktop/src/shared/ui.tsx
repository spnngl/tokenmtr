import { ArrowDown, ArrowUp, ArrowUpDown } from "lucide-react";
import { type ReactNode, useId, useState } from "react";
import { type Currency, formatCurrencyMicros } from "./format";

/** Screen title and one line of purpose. */
export function PageHeader(props: {
  readonly title: string;
  readonly summary: string;
  readonly actions?: ReactNode;
}): ReactNode {
  return (
    <header className="flex flex-wrap items-start justify-between gap-3">
      <div className="flex min-w-0 flex-col gap-1">
        <h1 className="text-xl font-semibold">{props.title}</h1>
        <p className="text-sm text-fg-muted">{props.summary}</p>
      </div>
      {props.actions}
    </header>
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

/** One headline figure. `hint` carries the qualifier (e.g. "estimated"). */
export function KpiTile(props: {
  readonly label: string;
  readonly value: ReactNode;
  readonly hint?: ReactNode;
}): ReactNode {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-md border border-border bg-surface-raised p-3">
      <dt className="text-xs font-medium text-fg-muted">{props.label}</dt>
      <dd className="truncate text-xl font-semibold tabular-nums">{props.value}</dd>
      {props.hint === undefined ? null : <dd className="text-xs text-fg-muted">{props.hint}</dd>}
    </div>
  );
}

/** A grid of `KpiTile`s: a single description list so screen readers announce it as one. */
export function KpiGrid({ children }: { readonly children: ReactNode }): ReactNode {
  return <dl className="grid grid-cols-2 gap-3 md:grid-cols-4">{children}</dl>;
}

/**
 * A chart with its table alternative (PLAN §7: every chart has one). The chart is decorative to
 * assistive tech (`aria-hidden`); the table carries the data.
 */
export function ChartFrame(props: {
  readonly title: string;
  readonly chart: ReactNode;
  readonly table: ReactNode;
  readonly actions?: ReactNode;
}): ReactNode {
  return (
    <Card title={props.title} actions={props.actions}>
      <div aria-hidden className="h-56 min-w-0">
        {props.chart}
      </div>
      <details className="text-sm">
        <summary className="cursor-pointer font-medium text-accent">View as table</summary>
        <div className="pt-2">{props.table}</div>
      </details>
    </Card>
  );
}

/** A money figure; `approximate` adds a visible and a spoken qualifier. */
export function Amount(props: {
  readonly micros: number;
  readonly currency: Currency;
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
  readonly id: string;
  readonly header: string;
  readonly cell: (row: Row) => ReactNode;
  /** Present when the column can be sorted. */
  readonly sortValue?: (row: Row) => number | string;
  readonly align?: "end";
}

type SortState = { readonly id: string; readonly direction: "asc" | "desc" } | null;

function compare(a: number | string, b: number | string): number {
  if (typeof a === "number" && typeof b === "number") {
    return a - b;
  }
  return String(a).localeCompare(String(b));
}

/** A semantic table; sortable columns toggle through header buttons (`aria-sort`). */
export function DataTable<Row>(props: {
  readonly caption: string;
  readonly columns: readonly Column<Row>[];
  readonly rows: readonly Row[];
  /** Unique among rows; `index` is for data with no natural key. */
  readonly rowKey: (row: Row, index: number) => string;
}): ReactNode {
  const [sort, setSort] = useState<SortState>(null);
  const sortColumn = props.columns.find((column) => column.id === sort?.id);
  const sortValue = sortColumn?.sortValue;
  const rows =
    sort === null || sortValue === undefined
      ? props.rows
      : [...props.rows].sort(
          (a, b) => compare(sortValue(a), sortValue(b)) * (sort.direction === "asc" ? 1 : -1),
        );

  const toggle = (id: string): void =>
    setSort((current) => {
      if (current?.id !== id) {
        return { id, direction: "desc" };
      }
      return current.direction === "desc" ? { id, direction: "asc" } : null;
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
              const active = sort?.id === column.id ? sort.direction : null;
              return (
                <th
                  key={column.id}
                  scope="col"
                  aria-sort={
                    active === null ? undefined : active === "asc" ? "ascending" : "descending"
                  }
                  className={`px-3 py-2 font-medium ${column.align === "end" ? "text-right" : ""}`}
                >
                  {column.sortValue === undefined ? (
                    column.header
                  ) : (
                    <button
                      type="button"
                      onClick={() => toggle(column.id)}
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
                  key={column.id}
                  className={`px-3 py-2 tabular-nums ${column.align === "end" ? "text-right" : ""}`}
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
