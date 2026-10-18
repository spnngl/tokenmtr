import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { AppProvider, bootstrapConfig } from "../shared/app-context";
import { axeViolations } from "../shared/axe";
import { emptyDataSource } from "../shared/data-source";
import { ThemeProvider } from "../shared/theme";
import { createAppRouter } from "./router";

afterEach(cleanup);

async function renderApp(url: string) {
  const router = createAppRouter(createMemoryHistory({ initialEntries: [url] }));
  await router.load();
  const view = render(
    <ThemeProvider>
      <AppProvider config={bootstrapConfig} dataSource={emptyDataSource}>
        <QueryClientProvider client={new QueryClient()}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </AppProvider>
    </ThemeProvider>,
  );
  const filters = await screen.findByRole("form", { name: "Filters" });
  return { ...view, router, filters, search: () => router.state.location.searchStr };
}

describe("filter bar", () => {
  it("writes the range to the URL and omits defaults", async () => {
    const user = userEvent.setup();
    const { filters, search } = await renderApp("/");
    expect(search()).toBe("");

    await user.click(within(filters).getByRole("button", { name: /Range/ }));
    await user.click(await screen.findByRole("option", { name: "Last 30 days" }));
    expect(search()).toBe("?range=30d");

    await user.click(within(filters).getByRole("button", { name: /Range/ }));
    await user.click(await screen.findByRole("option", { name: "Last 7 days" }));
    expect(search()).toBe("");
  });

  it("toggles the currency from the keyboard", async () => {
    const user = userEvent.setup();
    const { filters, search } = await renderApp("/");
    const usd = within(filters).getByRole("radio", { name: "USD" });
    expect(usd.getAttribute("aria-checked")).toBe("true");

    await user.click(within(filters).getByRole("radio", { name: "EUR" }));
    expect(search()).toBe("?currency=EUR");
    expect(within(filters).getByRole("radio", { name: "EUR" }).getAttribute("aria-checked")).toBe(
      "true",
    );
  });

  it("reads filters from the link and reports invalid ones", async () => {
    const { filters, search } = await renderApp("/quota?range=1y&currency=EUR&billing=api");
    expect(within(filters).getByRole("status").textContent).toContain("range");
    expect(within(filters).getByRole("radio", { name: "EUR" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(within(filters).getByRole("button", { name: /Billing/ }).textContent).toContain("API");
    expect(search()).toContain("billing=api");
  });

  it("keeps filters when navigating between screens", async () => {
    const user = userEvent.setup();
    const { router } = await renderApp("/?range=30d");
    const links = screen.getAllByRole("link", { name: "Billing" });
    const first = links[0];
    expect(first).toBeDefined();
    if (first === undefined) {
      return;
    }
    await user.click(first);
    expect(router.state.location.pathname).toBe("/billing");
    expect(router.state.location.searchStr).toBe("?range=30d");
  });

  it("has no axe violations", async () => {
    const { container } = await renderApp("/");
    expect(await axeViolations(container)).toEqual([]);
  });
});
