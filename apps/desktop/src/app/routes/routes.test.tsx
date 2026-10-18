import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { bootstrapConfig } from "../../shared/app-context";
import { AppProvider } from "../../shared/app-provider";
import { axeViolations } from "../../shared/axe";
import { createMockDataSource, type MockScenario } from "../../shared/mock";
import { BillingRoute } from "./billing";
import { ExportRoute } from "./export";
import { InsightsRoute } from "./insights";
import { ModelsRoute } from "./models";
import { ReportsRoute } from "./reports";

afterEach(cleanup);

/** Mounts one route component under a memory router with the mock data source. */
async function mount(
  component: () => ReactNode,
  scenario: MockScenario,
  onAction: (action: string) => void = () => undefined,
) {
  const rootRoute = createRootRoute({ component: Outlet });
  const route = createRoute({ getParentRoute: () => rootRoute, path: "/", component });
  const router = createRouter({
    routeTree: rootRoute.addChildren([route]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  await router.load();
  return render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <AppProvider
        config={{ ...bootstrapConfig, timeZone: "Europe/Paris" }}
        dataSource={createMockDataSource({ scenario, onAction })}
      >
        <RouterProvider router={router} />
      </AppProvider>
    </QueryClientProvider>,
  );
}

const ROUTES = [
  { name: "Billing", component: BillingRoute },
  { name: "Insights", component: InsightsRoute },
  { name: "Models", component: ModelsRoute },
  { name: "Reports", component: ReportsRoute },
  { name: "Export", component: ExportRoute },
] as const;

describe.each(ROUTES)("$name route states", ({ component }) => {
  it("loading", async () => {
    await mount(component, "loading");
    expect((await screen.findByRole("status")).getAttribute("aria-busy")).toBe("true");
  });

  it("error offers Retry", async () => {
    await mount(component, "error");
    expect(await screen.findByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("populated is accessible", async () => {
    const { container } = await mount(component, "populated");
    await screen.findByRole("heading", { level: 1 });
    await waitFor(() => expect(screen.queryByRole("status", { busy: true })).toBeNull());
    // Frames are out of axe's reach in jsdom.
    for (const frame of container.querySelectorAll("iframe")) {
      frame.remove();
    }
    expect(await axeViolations(container)).toEqual([]);
  });
});

describe("routes over the seeded mock", () => {
  it("Billing shows a budget over 100% and the advisor estimate label", async () => {
    await mount(BillingRoute, "populated");
    expect(await screen.findByText(/Over budget by/)).toBeTruthy();
    expect(screen.getAllByText("Estimate").length).toBeGreaterThan(0);
  });

  it("Insights lists MCP tools and the per-day figures stay under 400 points", async () => {
    const { container } = await mount(InsightsRoute, "populated");
    expect((await screen.findAllByText(/create_issue/)).length).toBeGreaterThan(0);
    for (const figure of container.querySelectorAll("figure[data-points]")) {
      expect(Number(figure.getAttribute("data-points"))).toBeLessThanOrEqual(400);
    }
  });

  it("Models lists the seeded models", async () => {
    await mount(ModelsRoute, "populated");
    expect(
      await screen.findByRole("table", { name: "Usage per model", hidden: true }),
    ).toBeTruthy();
  });

  it("Reports previews Markdown from the seeded queries", async () => {
    await mount(ReportsRoute, "populated");
    const preview = await screen.findByLabelText("Report preview");
    expect(preview.textContent).toContain("# TokenMtr usage report");
    expect(preview.textContent).toContain("## Per day");
  });

  it("Reports saves through the data source and shows the saved path", async () => {
    const actions: string[] = [];
    await mount(ReportsRoute, "populated", (action) => actions.push(action));
    await userEvent.setup().click(await screen.findByRole("button", { name: "Save Markdown" }));
    await waitFor(() => expect(actions).toContain("saveReport"));
    expect((await screen.findByRole("status")).textContent).toMatch(/^Saved to .*\.md$/);
  });

  it("Export now calls the data source", async () => {
    const actions: string[] = [];
    await mount(ExportRoute, "populated", (action) => actions.push(action));
    await userEvent.setup().click(await screen.findByRole("button", { name: "Export now" }));
    await waitFor(() => expect(actions).toContain("exportNow"));
  });
});
