import type { ReactNode } from "react";
import { useInstallDaemon, useRestartDaemon, useScanNow } from "../shared/actions";
import { useAppConfig } from "../shared/app-context";
import { errorText } from "../shared/errors";
import { formatDuration } from "../shared/format";
import { useAppState } from "../shared/queries";
import type { AppState } from "../shared/schemas";
import { StateView, type ViewState } from "../shared/state-view";
import { Onboarding } from "./onboarding";

type ReadyState = Extract<AppState, { kind: "ready" }>;

/** A scan older than twice the interval means the daemon is not keeping up (PLAN §7 "stale"). */
const STALE_FACTOR = 2;

/** `null` while the data is fresh, or when no scan ever ran (the empty state explains that). */
export function staleSince(state: ReadyState, nowMs: number): string | null {
  if (state.last_scan_at === null) {
    return null;
  }
  const ageSecs = Math.max(0, Math.round((nowMs - Date.parse(state.last_scan_at)) / 1000));
  return ageSecs > STALE_FACTOR * state.scan_interval_secs
    ? `${formatDuration(ageSecs)} ago`
    : null;
}

/**
 * Replaces the screens when the app cannot show data: loading, failed, daemon offline, daemon
 * version mismatch (D52) and first run. Otherwise renders `children`.
 */
export function AppGate({ children }: { readonly children: ReactNode }): ReactNode {
  const query = useAppState();
  const startDaemon = useInstallDaemon();
  const restartDaemon = useRestartDaemon();
  const state = query.data;

  if (state === undefined) {
    const view: ViewState = query.isError
      ? { kind: "error", ...errorText(query.error), onRetry: () => void query.refetch() }
      : { kind: "loading" };
    return (
      <div className="p-4 md:p-6">
        <StateView state={view} />
      </div>
    );
  }
  switch (state.kind) {
    case "ready":
      return children;
    case "onboarding":
      return <Onboarding state={state} />;
    case "daemon_offline":
      return (
        <div className="p-4 md:p-6">
          <StateView
            state={{
              kind: "daemon-offline",
              onStartDaemon: () => startDaemon.mutate(),
              isPending: startDaemon.isPending,
            }}
          />
        </div>
      );
    case "incompatible":
      return (
        <div className="p-4 md:p-6">
          <StateView
            state={{
              kind: "incompatible",
              appVersion: state.app_version,
              daemonVersion: state.daemon_version,
              onRestartDaemon: () => restartDaemon.mutate(),
              isPending: restartDaemon.isPending,
            }}
          />
        </div>
      );
    default:
      state satisfies never;
      throw new Error("unreachable app state");
  }
}

/** Stale and partial-data banners above the routed screen (the data is still shown). */
export function AppBanners({ children }: { readonly children: ReactNode }): ReactNode {
  const { data } = useAppState();
  const { now } = useAppConfig();
  const scan = useScanNow();
  if (data === undefined || data.kind !== "ready") {
    return children;
  }
  const since = staleSince(data, now());
  const stale: ViewState =
    since === null
      ? { kind: "ready" }
      : {
          kind: "stale",
          lastScan: since,
          onScanNow: () => scan.mutate(),
          isPending: scan.isPending,
        };
  const partial: ViewState =
    data.failed_collectors === 0
      ? { kind: "ready" }
      : { kind: "partial", failedCollectors: data.failed_collectors, issuesHref: "#/settings" };
  return (
    <StateView state={stale}>
      <StateView state={partial}>{children}</StateView>
    </StateView>
  );
}
