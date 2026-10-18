import {
  createRootRoute,
  createRoute,
  createRouter,
  type RouterHistory,
} from "@tanstack/react-router";
import type { ReactNode } from "react";
import { NAV_ITEMS, type NavPath } from "../shared/nav";
import { Layout } from "./layout";
import { BillingRoute } from "./routes/billing";
import { ExportRoute } from "./routes/export";
import { InsightsRoute } from "./routes/insights";
import { ModelsRoute } from "./routes/models";
import { ReportsRoute } from "./routes/reports";
import {
  OverviewRoute,
  ProjectDetailRoute,
  ProjectsRoute,
  QuotaRoute,
  SessionDetailRoute,
  SessionsRoute,
  SettingsRoute,
} from "./routes/screens";

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

/** Every nav path needs a component: the compiler flags a screen without one. */
const SCREENS: Record<NavPath, () => ReactNode> = {
  "/": OverviewRoute,
  "/quota": QuotaRoute,
  "/billing": BillingRoute,
  "/sessions": SessionsRoute,
  "/insights": InsightsRoute,
  "/models": ModelsRoute,
  "/projects": ProjectsRoute,
  "/reports": ReportsRoute,
  "/export": ExportRoute,
  "/settings": SettingsRoute,
};

const screenRoutes = NAV_ITEMS.map((item) =>
  createRoute({ getParentRoute: () => rootRoute, path: item.to, component: SCREENS[item.to] }),
);

const sessionDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sessions/$agent/$session",
  component: SessionDetailRoute,
});

const projectDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/projects/$project",
  component: ProjectDetailRoute,
});

const routeTree = rootRoute.addChildren([...screenRoutes, sessionDetailRoute, projectDetailRoute]);

/** One route per screen in `NAV_ITEMS`. Tests pass a memory history. Return type is inferred: it is the router's full generic type. */
export function createAppRouter(history: RouterHistory) {
  return createRouter({ routeTree, history, parseSearch, stringifySearch });
}
