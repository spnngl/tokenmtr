import type { ReactNode } from "react";

/** Monday first, like the backend's `weekday` 1-7. */
export const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;
const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

/** Seven rows (Monday first) of 24 hourly values, plus the peak that scales the fill. */
export interface WeekHourGrid {
  readonly rows: readonly (readonly number[])[];
  readonly max: number;
}

/** Folds `(weekday 1-7, hour 0-23, value)` cells into a grid; missing cells are 0. */
export function weekHourGrid<Cell extends { readonly weekday: number; readonly hour: number }>(
  cells: readonly Cell[],
  value: (cell: Cell) => number,
): WeekHourGrid {
  const rows = WEEKDAYS.map(() => new Array<number>(24).fill(0));
  for (const cell of cells) {
    const row = rows[cell.weekday - 1];
    if (row !== undefined) {
      row[cell.hour] = value(cell);
    }
  }
  return { rows, max: Math.max(0, ...rows.flat()) };
}

/** Fill strength 0-1 of a cell; zero activity is exactly 0. */
export function heatIntensity(value: number, max: number): number {
  return max <= 0 || value <= 0 ? 0 : value / max;
}

/** Fill of a cell: the accent over the surface, in proportion to the cell's share of the peak. */
function cellStyle(intensity: number): { backgroundColor: string } {
  return {
    backgroundColor: `color-mix(in srgb, var(--accent) ${Math.round(intensity * 100)}%, var(--surface-muted))`,
  };
}

/**
 * Weekday by hour heatmap as a real table, so a screen reader gets every value; intensity is a
 * visual aid and every cell also carries its value as text.
 */
export function WeekHourTable({
  label,
  grid,
  format,
}: {
  /** Names the scroll region and the table. */
  readonly label: string;
  readonly grid: WeekHourGrid;
  readonly format: (value: number) => string;
}): ReactNode {
  return (
    <section
      aria-label={label}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll region must be focusable (WCAG 2.1.1).
      tabIndex={0}
      className="overflow-x-auto rounded-md border border-border"
    >
      <table className="border-collapse text-xs" data-points={WEEKDAYS.length * HOURS.length}>
        <caption className="sr-only">{label}</caption>
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
                const text = format(value);
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
  );
}
