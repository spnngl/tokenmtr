import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { bootstrapConfig } from "../../shared/app-context";
import { AppProvider } from "../../shared/app-provider";
import { axeViolations } from "../../shared/axe";
import { usage } from "../../shared/fixtures";
import { createMockDataSource, type MockScenario } from "../../shared/mock";
import type { Series } from "../../shared/schemas";
import { ProjectCostLines, seriesLabels } from "./project-charts";
import { ProjectDetailScreen, ProjectsScreen } from "./projects-screen";

afterEach(cleanup);

/** Mounts one screen under a memory router with the mock data source. */
async function mount(component: () => ReactNode, scenario: MockScenario) {
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
        dataSource={createMockDataSource({ scenario })}
      >
        <RouterProvider router={router} />
      </AppProvider>
    </QueryClientProvider>,
  );
}

const SCREENS = [
  {
    name: "Projects",
    component: ProjectsScreen,
    charts: ["Cost over time", "Cost by model per project"],
  },
  {
    name: "Project detail",
    component: () => <ProjectDetailScreen project="tokenmtr" />,
    charts: ["Cost by model", "API value per model over time"],
  },
] as const;

describe.each(SCREENS)("$name screen", ({ component, charts }) => {
  it("shows its loading state", async () => {
    await mount(component, "loading");
    expect((await screen.findAllByRole("status"))[0]?.getAttribute("aria-busy")).toBe("true");
  });

  it("offers Retry on error", async () => {
    await mount(component, "error");
    expect((await screen.findAllByRole("button", { name: "Retry" })).length).toBeGreaterThan(0);
  });

  it("draws no chart without usage", async () => {
    await mount(component, "empty");
    await screen.findByRole("heading", { level: 1 });
    await waitFor(() => expect(screen.queryByRole("status", { busy: true })).toBeNull());
    expect(screen.queryByRole("figure")).toBeNull();
  });

  it("draws each chart with its table, accessibly", async () => {
    const { container } = await mount(component, "populated");
    const user = userEvent.setup();
    for (const name of charts) {
      const figure = await screen.findByRole("figure", { name });
      await user.click(within(figure).getByText("View as table"));
      expect(within(figure).getByRole("table")).toBeTruthy();
    }
    expect(await axeViolations(container)).toEqual([]);
  });
});

describe("cost over time per project", () => {
  const keys = ["(none)", ...Array.from({ length: 9 }, (_, index) => `p${index}`)];
  const series: Series = {
    currency: "EUR",
    bucket: "day",
    group_by: "project",
    points: ["2026-05-01", "2026-05-02"].map((start) => ({
      start,
      minutes: 1440,
      ...usage({}),
      cumulative_api_value_micros: 0,
      cumulative_paid_micros: 0,
      groups: Object.fromEntries(
        keys.map((key, rank) => [
          key,
          { total_tokens: 1, api_value_micros: (keys.length - rank) * 1_000_000, paid_micros: 0 },
        ]),
      ),
    })),
  };

  it("draws the eight costliest projects by name and adds the rest up to Other", async () => {
    const label = seriesLabels([
      { ...usage({}), project: "p0", name: "Zero", client: null, share_percent: 0 },
    ]);
    render(<ProjectCostLines series={series} label={label} />);
    await userEvent.setup().click(screen.getByText("View as table"));
    const table = screen.getByRole("table", { name: "Cost over time per project" });
    const headers = within(table)
      .getAllByRole("columnheader")
      .map((header) => header.textContent);
    expect(headers).toContain("No project");
    expect(headers).toContain("Zero");
    expect(headers).toContain("Other");
    expect(headers).not.toContain("p8");
  });
});
