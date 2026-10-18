import type { ReactNode } from "react";
import { StateView, type ViewState } from "../../shared/state-view";

/** Where a screen's data stands. The route layer maps its query hook onto this. */
export type Remote<T> =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly message: string; readonly onRetry: () => void }
  | { readonly kind: "daemon-offline"; readonly onStartDaemon: () => void }
  | { readonly kind: "ready"; readonly data: T };

interface EmptyCopy {
  readonly title: string;
  readonly hint: string;
}

interface RemoteViewProps<T> {
  readonly remote: Remote<T>;
  /** When it returns an `EmptyCopy` the screen shows the empty state with that setup hint. */
  readonly emptyWhen: (data: T) => EmptyCopy | null;
  readonly children: (data: T) => ReactNode;
}

/** Maps `Remote` onto the shared `StateView` (§7 states) and renders `children` when populated. */
export function RemoteView<T>({ remote, emptyWhen, children }: RemoteViewProps<T>): ReactNode {
  switch (remote.kind) {
    case "loading":
      return <StateView state={{ kind: "loading" }}>{null}</StateView>;
    case "error":
      return <StateView state={remote}>{null}</StateView>;
    case "daemon-offline":
      return <StateView state={remote}>{null}</StateView>;
    case "ready": {
      const empty = emptyWhen(remote.data);
      const state: ViewState =
        empty === null
          ? { kind: "ready" }
          : { kind: "empty", title: empty.title, hint: empty.hint };
      return <StateView state={state}>{children(remote.data)}</StateView>;
    }
    default:
      return remote satisfies never;
  }
}
