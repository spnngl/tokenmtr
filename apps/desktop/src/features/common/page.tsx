import type { ReactNode } from "react";
import { NAV_ITEMS } from "../../app/nav";

/** Title and one-line summary of a screen, read from the nav so the text lives in one place. */
export function PageHeader({ to }: { readonly to: string }): ReactNode {
  const item = NAV_ITEMS.find((candidate) => candidate.to === to);
  if (item === undefined) {
    throw new Error(`no nav item for ${to}`);
  }
  return (
    <header className="flex flex-col gap-1">
      <h1 className="text-xl font-semibold">{item.label}</h1>
      <p className="text-sm text-fg-muted">{item.summary}</p>
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

interface KpiProps {
  readonly label: string;
  readonly value: ReactNode;
  readonly detail?: ReactNode;
}

/** One headline figure. Wrap several in `KpiGrid`. */
export function Kpi({ label, value, detail }: KpiProps): ReactNode {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-md border border-border bg-surface-raised p-3">
      <dt className="text-sm text-fg-muted">{label}</dt>
      <dd className="text-xl font-semibold tabular-nums">{value}</dd>
      {detail === undefined ? null : <dd className="text-xs text-fg-muted">{detail}</dd>}
    </div>
  );
}

export function KpiGrid({ children }: { readonly children: ReactNode }): ReactNode {
  return <dl className="grid grid-cols-2 gap-3 md:grid-cols-4">{children}</dl>;
}
