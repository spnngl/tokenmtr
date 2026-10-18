import type { QueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { EVENTS } from "./bindings";

/** Tauri's `listen`, reduced to the payload: resolves to the function that unsubscribes. */
export type Listen = (event: string, handler: (payload: unknown) => void) => Promise<() => void>;

/** `daemon-state` payload: what the shell's control-socket handshake last found (D52). */
export const daemonStateSchema = z.discriminatedUnion("state", [
  z.object({ state: z.literal("connected"), daemon_version: z.string() }),
  z.object({ state: z.literal("offline") }),
  z.object({ state: z.literal("incompatible"), daemon_version: z.string() }),
]);
export type DaemonState = z.infer<typeof daemonStateSchema>;

/** `navigate` payload: where the already open main window should go. */
export const navigateSchema = z.object({
  route: z.string().startsWith("/"),
  agent: z.string().nullable(),
});

/** Path and search of a `navigate` payload, as the router's history takes it. */
export function navigationPath(target: z.infer<typeof navigateSchema>): string {
  return target.agent === null
    ? target.route
    : `${target.route}?agent=${encodeURIComponent(target.agent)}`;
}

export interface ShellEventOptions {
  readonly listen: Listen;
  readonly queryClient: Pick<QueryClient, "invalidateQueries">;
  /**
   * `main` follows every push. `recap` is hidden most of the time and re-queries only when the
   * shell shows it (D47), so it ignores `store-changed`.
   */
  readonly window: "main" | "recap";
  /** Opens a path in the main window's router; only the main window handles `navigate`. */
  readonly navigate?: (path: string) => void;
}

/**
 * Wires the shell's pushed events to the query cache: a store change refetches what is on
 * screen, a daemon state change refetches the app state, a navigation moves the router. Resolves
 * to a function that removes every listener.
 */
export async function subscribeToShell(options: ShellEventOptions): Promise<() => void> {
  const { listen, queryClient, navigate } = options;
  const everything = () => queryClient.invalidateQueries();
  const subscriptions: Promise<() => void>[] = [];

  if (options.window === "main") {
    subscriptions.push(
      listen(EVENTS.storeChanged, everything),
      listen(EVENTS.alertFired, () => {
        void queryClient.invalidateQueries({ queryKey: ["alerts"] });
        void queryClient.invalidateQueries({ queryKey: ["system-health"] });
      }),
      listen(EVENTS.navigate, (payload) => {
        const target = navigateSchema.safeParse(payload);
        if (target.success) {
          navigate?.(navigationPath(target.data));
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

  const unsubscribers = await Promise.all(subscriptions);
  return () => {
    for (const unsubscribe of unsubscribers) {
      unsubscribe();
    }
  };
}
