import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render } from "@testing-library/react";
import { bootstrapConfig } from "../shared/app-context";
import { AppProvider } from "../shared/app-provider";
import type { DataSource } from "../shared/data-source";
import { createMockDataSource, type MockOptions } from "../shared/mock";
import { NOW_MS } from "../shared/mock/seed";
import { ThemeProvider } from "../shared/theme";
import { createAppRouter } from "./router";

export type Harness = Omit<MockOptions, "latencyMs"> & {
  /** Replaces some reads or actions of the mock, e.g. to make one of them fail. */
  readonly wrap?: (dataSource: DataSource) => DataSource;
};

/** The whole app (router, layout, gate) on the mock data source with a fixed clock and zone. */
export async function renderApp(url: string, { wrap, ...options }: Harness = {}) {
  const mock = createMockDataSource(options);
  const router = createAppRouter(createMemoryHistory({ initialEntries: [url] }));
  await router.load();
  const view = render(
    <ThemeProvider>
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
      >
        <AppProvider
          config={{ ...bootstrapConfig, timeZone: "Europe/Paris", now: () => NOW_MS }}
          dataSource={wrap === undefined ? mock : wrap(mock)}
        >
          <RouterProvider router={router} />
        </AppProvider>
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return { ...view, router };
}
