import type { UseQueryResult } from "@tanstack/react-query";
import { errorText } from "./errors";
import type { Remote } from "./remote-view";

/** Maps one query onto `Remote`; an error carries the user-facing text of its kind. */
export function toRemote<T>(query: UseQueryResult<T>): Remote<T> {
  switch (query.status) {
    case "pending":
      return { kind: "loading" };
    case "error":
      return {
        kind: "error",
        ...errorText(query.error),
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
