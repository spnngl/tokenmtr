import type { UseQueryResult } from "@tanstack/react-query";
import { useDataSource } from "../../shared/app-context";
import { useAppState } from "../../shared/queries";
import type { Remote } from "./remote-view";

/** Maps one query onto `Remote`; `offline` (daemon not running) wins over the query's own state. */
export function toRemote<T>(query: UseQueryResult<T>, offline: Remote<never> | null): Remote<T> {
  if (offline !== null && offline.kind !== "ready") {
    return offline;
  }
  switch (query.status) {
    case "pending":
      return { kind: "loading" };
    case "error":
      return {
        kind: "error",
        message: query.error.message,
        onRetry: () => {
          void query.refetch();
        },
      };
    case "success":
      return { kind: "ready", data: query.data };
    default:
      return query satisfies never;
  }
}

/** Both ready gives `join`'s result; otherwise the first blocking state. */
export function joinRemotes<A, B, R>(
  a: Remote<A>,
  b: Remote<B>,
  join: (a: A, b: B) => R,
): Remote<R> {
  if (a.kind !== "ready") {
    return a;
  }
  if (b.kind !== "ready") {
    return b;
  }
  return { kind: "ready", data: join(a.data, b.data) };
}

/**
 * The state shared by every screen: `daemon-offline` with a start action when the app reports the
 * daemon is down, else `null`.
 */
export function useDaemonGate(): Remote<never> | null {
  const dataSource = useDataSource();
  const state = useAppState();
  if (state.data?.kind !== "daemon_offline") {
    return null;
  }
  return {
    kind: "daemon-offline",
    onStartDaemon: () => {
      void dataSource.installDaemon();
    },
  };
}
