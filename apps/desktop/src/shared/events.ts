import type { QueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { EVENTS } from "./bindings";
import { isNavPath, type NavPath } from "./nav";

/** Tauri's `listen`, reduced to the payload: resolves to the function that unsubscribes. */
export type Listen = (event: string, handler: (payload: unknown) => void) => Promise<() => void>;

/** `daemon-state` payload: what the shell's control-socket handshake last found. */
export const daemonStateSchema = z.discriminatedUnion("state", [
  z.object({ state: z.literal("connected"), daemon_version: z.string() }),
  z.object({ state: z.literal("offline") }),
  z.object({
    state: z.literal("incompatible"),
    daemon_version: z.string(),
    daemon_schema: z.number().int().nonnegative(),
  }),
]);
export type DaemonState = z.infer<typeof daemonStateSchema>;

/** `navigate` payload: where the already open main window should go. */
const navigateSchema = z.object({
  route: z.custom<NavPath>((value) => typeof value === "string" && isNavPath(value)),
  agent: z.string().nullable(),
});
export type NavigationTarget = z.infer<typeof navigateSchema>;

/**
 * The search of a `navigate` target: the current one (the user's filters survive the jump), with
 * the target's agent when it names one.
 */
export function navigationSearch(
  current: Readonly<Record<string, unknown>>,
  agent: string | null,
): Record<string, unknown> {
  return agent === null ? { ...current } : { ...current, agent };
}

/**
 * `Dataset` of `store-changed` (`crates/tokenmtr-control/src/protocol.rs`) to the first segment
 * of the query keys whose answers read it. Health and the app state follow every write.
 */
const DATASET_QUERIES: Readonly<Record<string, readonly string[]>> = {
  usage_events: [
    "filter-options",
    "overview",
    "series",
    "models",
    "projects",
    "branches",
    "unresolved-cwds",
    "heatmap",
    "signals",
    "billing",
    "sessions",
    "session-detail",
    "session-origin",
    "advisor",
    "budgets",
    "recap",
  ],
  tool_calls: ["tools", "signals", "session-detail", "recap"],
  subscriptions: ["overview", "series", "billing", "advisor", "budgets", "recap"],
  quota_snapshots: ["quota-latest", "quota-history", "advisor", "recap"],
  fx_rates: [
    "settings",
    "overview",
    "series",
    "models",
    "projects",
    "branches",
    "heatmap",
    "billing",
    "sessions",
    "session-detail",
    "advisor",
    "budgets",
    "recap",
  ],
};
const ALWAYS = ["app-state", "system-health", "parse-issues", "alerts"] as const;
const ALL_STORE_QUERIES: ReadonlySet<string> = new Set([
  ...ALWAYS,
  ...Object.values(DATASET_QUERIES).flat(),
]);

const storeChangedSchema = z.object({ datasets: z.array(z.string()) });

/**
 * Query-key heads a `store-changed` payload invalidates. An empty list (sent after a reconnect),
 * an unknown dataset or an unreadable payload refresh every store-backed query. Report previews
 * are never refetched by a push: the user regenerates them.
 */
export function queriesForStoreChange(payload: unknown): ReadonlySet<string> {
  const parsed = storeChangedSchema.safeParse(payload);
  if (!parsed.success || parsed.data.datasets.length === 0) {
    return ALL_STORE_QUERIES;
  }
  const heads = new Set<string>(ALWAYS);
  for (const dataset of parsed.data.datasets) {
    const queries = DATASET_QUERIES[dataset];
    if (queries === undefined) {
      return ALL_STORE_QUERIES;
    }
    for (const head of queries) {
      heads.add(head);
    }
  }
  return heads;
}

export interface ShellEventOptions {
  readonly listen: Listen;
  readonly queryClient: Pick<QueryClient, "invalidateQueries">;
  /**
   * `main` follows every push. `recap` is hidden most of the time and re-queries only when the
   * shell shows it, so it ignores `store-changed`.
   */
  readonly window: "main" | "recap";
  /** Moves the main window's router; only the main window handles `navigate`. */
  readonly navigate?: (target: NavigationTarget) => void;
}

/**
 * Wires the shell's pushed events to the query cache: a store change refetches the queries that
 * read the changed datasets, a daemon state change refetches the app state, a navigation moves
 * the router. Resolves to a function that removes every listener; a listener that fails to
 * register is logged and skipped, never leaked.
 */
export async function subscribeToShell(options: ShellEventOptions): Promise<() => void> {
  const { listen, queryClient, navigate } = options;
  const everything = () => queryClient.invalidateQueries();
  const subscriptions: Promise<() => void>[] = [];

  if (options.window === "main") {
    subscriptions.push(
      listen(EVENTS.storeChanged, (payload) => {
        const heads = queriesForStoreChange(payload);
        void queryClient.invalidateQueries({
          predicate: (query) => heads.has(String(query.queryKey[0])),
        });
      }),
      listen(EVENTS.alertFired, () => {
        void queryClient.invalidateQueries({ queryKey: ["alerts"] });
        void queryClient.invalidateQueries({ queryKey: ["system-health"] });
      }),
      listen(EVENTS.navigate, (payload) => {
        const target = navigateSchema.safeParse(payload);
        if (target.success) {
          navigate?.(target.data);
        }
      }),
    );
  } else {
    subscriptions.push(listen(EVENTS.recapShown, everything));
  }
  subscriptions.push(
    listen(EVENTS.daemonState, (payload) => {
      const state = daemonStateSchema.safeParse(payload);
      if (!state.success) {
        return;
      }
      // A daemon that just came back has missed pushes: refetch everything, not only the gate.
      if (state.data.state === "connected") {
        void everything();
      } else {
        void queryClient.invalidateQueries({ queryKey: ["app-state"] });
      }
    }),
  );

  const results = await Promise.allSettled(subscriptions);
  const unsubscribers: (() => void)[] = [];
  for (const result of results) {
    if (result.status === "fulfilled") {
      unsubscribers.push(result.value);
    } else {
      console.error("could not listen to a shell event", result.reason);
    }
  }
  return () => {
    for (const unsubscribe of unsubscribers) {
      unsubscribe();
    }
  };
}
