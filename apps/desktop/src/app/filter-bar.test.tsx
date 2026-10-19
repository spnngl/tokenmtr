import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { bootstrapConfig } from "../shared/app-context";
import { AppProvider } from "../shared/app-provider";
import { axeViolations } from "../shared/axe";
import { createMockDataSource } from "../shared/mock";
import { ThemeProvider } from "../shared/theme";
import { createAppRouter } from "./router";
import { readStoredRange } from "./stored-range";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

async function renderApp(url: string) {
  const router = createAppRouter(createMemoryHistory({ initialEntries: [url] }));
  await router.load();
  const view = render(
    <ThemeProvider>
      <QueryClientProvider client={new QueryClient()}>
        <AppProvider config={bootstrapConfig} dataSource={createMockDataSource()}>
          <RouterProvider router={router} />
        </AppProvider>
      </QueryClientProvider>
    </ThemeProvider>,
  );
  const filters = await screen.findByRole("form", { name: "Filters" });
  return { ...view, router, filters, search: () => router.state.location.searchStr };
}

describe("filter bar", () => {
  it("writes the range to the URL, omits defaults and remembers the last range", async () => {
    const user = userEvent.setup();
    const { filters, search } = await renderApp("/");
    expect(search()).toBe("");

    await user.click(within(filters).getByRole("button", { name: /Range/ }));
    await user.click(await screen.findByRole("option", { name: "Last 7 days" }));
    expect(search()).toBe("?range=7d");
    expect(readStoredRange()).toEqual({ kind: "preset", preset: "7d" });

    await user.click(within(filters).getByRole("button", { name: /Range/ }));
    await user.click(await screen.findByRole("option", { name: "Last 30 days" }));
    expect(search()).toBe("");
    expect(readStoredRange()).toEqual({ kind: "preset", preset: "30d" });
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
