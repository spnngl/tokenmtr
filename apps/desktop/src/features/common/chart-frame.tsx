import { type ReactNode, useId } from "react";

/** Series colours: the theme's categorical chart tokens, in order. */
export const SERIES_COLORS = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
  "var(--chart-6)",
  "var(--chart-7)",
  "var(--chart-8)",
] as const;

/** Colour of the nth series; wraps after the eighth. */
export function seriesColor(index: number): string {
  return SERIES_COLORS[index % SERIES_COLORS.length] ?? "var(--chart-1)";
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
  readonly children: ReactNode;
}

/**
 * Accessible chart wrapper: a figure with a caption, a description, and the data as a table
 * behind a disclosure. The picture itself is hidden from assistive tech so the table, which
 * carries every value, is the only thing it reads.
 */
export function ChartFrame({
  title,
  description,
  points,
  table,
  children,
}: ChartFrameProps): ReactNode {
  const descriptionId = useId();
  return (
    <figure
      aria-label={title}
      aria-describedby={descriptionId}
      data-points={points}
      className="flex flex-col gap-2 rounded-md border border-border bg-surface-raised p-3"
    >
      <figcaption className="text-sm font-medium">{title}</figcaption>
      <p id={descriptionId} className="text-xs text-fg-muted">
        {description}
      </p>
      <div aria-hidden>{children}</div>
      <details className="text-sm">
        <summary className="cursor-pointer rounded-md py-1 text-fg-muted focus-visible:outline-2 focus-visible:outline-ring">
          View as table
        </summary>
        <div className="pt-2">{table}</div>
      </details>
    </figure>
  );
}
