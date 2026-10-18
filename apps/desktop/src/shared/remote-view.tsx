import type { ReactNode } from "react";
import { type StateSize, StateView, type ViewState } from "./state-view";
import { Card } from "./ui";

/**
 * Where a read stands: the one data-state model of every screen (`toRemote` maps a query onto
 * it). Daemon-level states (offline, incompatible) never reach a screen: `AppGate` replaces the
 * screens while they last.
 */
export type Remote<T> =
  | { readonly kind: "loading" }
  | {
      readonly kind: "error";
      readonly title: string;
      readonly message: string;
      readonly onRetry: () => void;
    }
  | { readonly kind: "ready"; readonly data: T };

/** Copy of an empty state: what is missing and how to get it. */
export interface EmptyCopy {
  readonly title: string;
  readonly hint: string;
}

/** Empty state of every screen that aggregates usage over the filtered range. */
export const NO_USAGE_COPY: EmptyCopy = {
  title: "No usage in this range",
  hint: "Widen the range or clear a filter. If you have not run an agent yet, start a session and scan.",
};

interface RemoteViewProps<T> {
  readonly remote: Remote<T>;
  /** When it returns an `EmptyCopy` the view shows the empty state with that hint. */
  readonly emptyWhen?: (data: T) => EmptyCopy | null;
  /** `section` for one card of a screen: its error stays inside the card. Default `page`. */
  readonly size?: StateSize | undefined;
  readonly children: (data: T) => ReactNode;
}

/**
 * Maps `Remote` onto the shared `StateView` states and renders `children` when populated.
 * An error is never shown as empty: it keeps its message and a retry.
 */
export function RemoteView<T>({
  remote,
  emptyWhen,
  size,
  children,
}: RemoteViewProps<T>): ReactNode {
  switch (remote.kind) {
    case "loading":
      return <StateView state={remote} size={size} />;
    case "error":
      return <StateView state={remote} size={size} />;
    case "ready": {
      const empty = emptyWhen?.(remote.data) ?? null;
      const state: ViewState = empty === null ? { kind: "ready" } : { kind: "empty", ...empty };
      return (
        <StateView state={state} size={size}>
          {children(remote.data)}
        </StateView>
      );
    }
    default:
      return remote satisfies never;
  }
}

/**
 * One card of a screen with its own read: while the read loads or fails, a titled card holds the
 * state (an error stays an error, with Retry); once ready, `children` renders the real card.
 */
export function CardRemote<T>(props: {
  readonly title: string;
  readonly remote: Remote<T>;
  readonly children: (data: T) => ReactNode;
}): ReactNode {
  const { title, remote, children } = props;
  if (remote.kind === "ready") {
    return children(remote.data);
  }
  return (
    <Card title={title}>
      <StateView state={remote} size="section" />
    </Card>
  );
}
