import {
  CircleAlert,
  Clock,
  Download,
  Inbox,
  LoaderCircle,
  ServerOff,
  TriangleAlert,
} from "lucide-react";
import type { ReactNode } from "react";
import { Link } from "react-aria-components";
import { APP_NAME } from "./brand";
import { Button } from "./button";

/**
 * The shared view states. `loading`, `empty`, `error` and `daemon-offline` replace the
 * content; `partial` and `stale` keep it and add a banner; `ready` is just the content.
 */
export type ViewState =
  | { readonly kind: "ready" }
  | { readonly kind: "loading" }
  | { readonly kind: "empty"; readonly title: string; readonly hint: string }
  | {
      readonly kind: "error";
      readonly title: string;
      readonly message: string;
      readonly onRetry: () => void;
    }
  | {
      readonly kind: "daemon-offline";
      readonly onStartDaemon: () => void;
      readonly isPending: boolean;
    }
  | {
      readonly kind: "incompatible";
      readonly appVersion: string;
      readonly daemonVersion: string;
      readonly onRestartDaemon: () => void;
      readonly isPending: boolean;
    }
  | { readonly kind: "partial"; readonly failedCollectors: number; readonly issuesHref: string }
  | {
      readonly kind: "stale";
      readonly lastScan: string;
      readonly onScanNow: () => void;
      readonly isPending: boolean;
    };

/** `page` fills a screen; `section` fits inside one card of a screen. */
export type StateSize = "page" | "section";

interface StateViewProps {
  readonly state: ViewState;
  readonly size?: StateSize | undefined;
  /** Content shown for `ready`, `partial` and `stale`. */
  readonly children?: ReactNode;
}

const PANEL_SIZE: Record<StateSize, string> = {
  page: "min-h-48 gap-3 rounded-md border border-border bg-surface-raised p-6",
  section: "min-h-24 gap-2 p-3",
};

function Panel(props: {
  readonly size: StateSize;
  readonly role: "status" | "alert";
  readonly icon: ReactNode;
  readonly title: string;
  readonly body?: string;
  readonly busy?: boolean;
  readonly action?: ReactNode;
}): ReactNode {
  return (
    <div
      role={props.role}
      aria-busy={props.busy}
      className={`flex flex-col items-center justify-center text-center ${PANEL_SIZE[props.size]}`}
    >
      {props.icon}
      <p className="text-base font-semibold">{props.title}</p>
      {props.body === undefined ? null : (
        <p className="max-w-prose text-sm text-fg-muted">{props.body}</p>
      )}
      {props.action}
    </div>
  );
}

function Banner(props: {
  readonly icon: ReactNode;
  readonly text: string;
  readonly action: ReactNode;
}): ReactNode {
  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border border-border bg-surface-muted px-3 py-2 text-sm"
    >
      {props.icon}
      <span className="min-w-0 flex-1">{props.text}</span>
      {props.action}
    </div>
  );
}

/** Renders one view state around the page content. */
export function StateView({ state, size = "page", children }: StateViewProps): ReactNode {
  switch (state.kind) {
    case "ready":
      return children;
    case "loading":
      return (
        <Panel
          size={size}
          role="status"
          busy
          icon={
            <LoaderCircle className="size-6 text-fg-muted motion-safe:animate-spin" aria-hidden />
          }
          title="Loading"
        />
      );
    case "empty":
      return (
        <Panel
          size={size}
          role="status"
          icon={<Inbox className="size-6 text-fg-muted" aria-hidden />}
          title={state.title}
          body={state.hint}
        />
      );
    case "error":
      return (
        <Panel
          size={size}
          role="alert"
          icon={<CircleAlert className="size-6 text-danger" aria-hidden />}
          title={state.title}
          body={state.message}
          action={
            <Button variant="primary" onPress={state.onRetry}>
              Retry
            </Button>
          }
        />
      );
    case "daemon-offline":
      return (
        <Panel
          size={size}
          role="alert"
          icon={<ServerOff className="size-6 text-warning" aria-hidden />}
          title="Daemon not running"
          body={`${APP_NAME} needs its background daemon to scan your agent sessions.`}
          action={
            <Button variant="primary" onPress={state.onStartDaemon} isPending={state.isPending}>
              Install / start daemon
            </Button>
          }
        />
      );
    case "incompatible":
      return (
        <Panel
          size={size}
          role="alert"
          icon={<Download className="size-6 text-warning" aria-hidden />}
          title="Update required"
          body={`The app is ${state.appVersion} but the daemon is ${state.daemonVersion}. Restart the daemon after updating so both run the same version.`}
          action={
            <Button variant="primary" onPress={state.onRestartDaemon} isPending={state.isPending}>
              Restart daemon
            </Button>
          }
        />
      );
    case "partial":
      return (
        <div className="flex flex-col gap-4">
          <Banner
            icon={<TriangleAlert className="size-4 shrink-0 text-warning" aria-hidden />}
            text={`${state.failedCollectors} ${state.failedCollectors === 1 ? "collector" : "collectors"} failed. Totals may be incomplete.`}
            action={
              <Link href={state.issuesHref} className="font-medium text-accent underline">
                View parse issues
              </Link>
            }
          />
          {children}
        </div>
      );
    case "stale":
      return (
        <div className="flex flex-col gap-4">
          <Banner
            icon={<Clock className="size-4 shrink-0 text-warning" aria-hidden />}
            text={`Last scan ${state.lastScan}. Data may be out of date.`}
            action={
              <Button onPress={state.onScanNow} isPending={state.isPending}>
                Scan now
              </Button>
            }
          />
          {children}
        </div>
      );
    default:
      state satisfies never;
      throw new Error("unreachable view state");
  }
}
