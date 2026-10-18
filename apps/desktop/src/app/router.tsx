import {
  createRootRoute,
  createRoute,
  createRouter,
  type RouterHistory,
} from "@tanstack/react-router";
import { Layout } from "./layout";
import { NAV_ITEMS } from "./nav";
import { PlaceholderPage } from "./placeholder-page";

/**
 * Search params stay plain strings: filters are parsed by `parseFilters`, so the router must not
 * JSON-decode them (its default turns `agent=123` into a number).
 */
function parseSearch(searchStr: string): Record<string, string> {
  return Object.fromEntries(new URLSearchParams(searchStr));
}

function stringifySearch(search: Record<string, unknown>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(search)) {
    if (typeof value === "string") {
      params.set(key, value);
    }
  }
  const text = params.toString();
  return text === "" ? "" : `?${text}`;
}

const rootRoute = createRootRoute({ component: Layout });

const routeTree = rootRoute.addChildren(
  NAV_ITEMS.map((item) =>
    createRoute({
      getParentRoute: () => rootRoute,
      path: item.to,
      component: () => <PlaceholderPage item={item} />,
    }),
  ),
);

/** One route per screen in `NAV_ITEMS`. Tests pass a memory history. Return type is inferred: it is the router's full generic type. */
export function createAppRouter(history: RouterHistory) {
  return createRouter({ routeTree, history, parseSearch, stringifySearch });
}
