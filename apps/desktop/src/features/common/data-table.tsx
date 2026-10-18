import type { ReactNode } from "react";

export interface Column<T> {
  readonly header: string;
  /** Numbers and money align to the end. */
  readonly numeric?: boolean;
  readonly cell: (row: T) => ReactNode;
}

interface DataTableProps<T> {
  /** Names the table for assistive tech; also the label of its scroll region. */
  readonly caption: string;
  readonly columns: readonly Column<T>[];
  readonly rows: readonly T[];
  readonly rowKey: (row: T) => string;
}

/**
 * A native table: the text alternative of every chart and the main view of tabular screens.
 * Wide tables scroll inside a focusable region, so keyboard users can reach every column.
 */
export function DataTable<T>({ caption, columns, rows, rowKey }: DataTableProps<T>): ReactNode {
  return (
    <section
      aria-label={caption}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll region must be focusable (WCAG 2.1.1).
      tabIndex={0}
      className="overflow-x-auto rounded-md border border-border"
    >
      <table className="w-full border-collapse text-left text-sm">
        <caption className="sr-only">{caption}</caption>
        <thead className="bg-surface-muted text-fg-muted">
          <tr>
            {columns.map((column) => (
              <th
                key={column.header}
                scope="col"
                className={`px-3 py-2 font-medium whitespace-nowrap ${column.numeric === true ? "text-right" : ""}`}
              >
                {column.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={rowKey(row)} className="border-t border-border">
              {columns.map((column) => (
                <td
                  key={column.header}
                  className={`px-3 py-2 ${column.numeric === true ? "text-right tabular-nums" : ""}`}
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
