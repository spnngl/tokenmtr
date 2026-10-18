import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render } from "@testing-library/react";
import { AppProvider, bootstrapConfig } from "../shared/app-context";
import { createMockDataSource, type MockScenario } from "../shared/mock";
import { NOW_MS } from "../shared/mock/seed";
import { ThemeProvider } from "../shared/theme";
import { createAppRouter } from "./router";

export interface Harness {
  readonly scenario?: MockScenario;
  readonly onAction?: (action: string, args: readonly unknown[]) => void;
}

/** The whole app (router, layout, gate) on the mock data source with a fixed clock and zone. */
export async function renderApp(url: string, options: Harness = {}) {
  const router = createAppRouter(createMemoryHistory({ initialEntries: [url] }));
  await router.load();
  const view = render(
    <ThemeProvider>
      <AppProvider
        config={{ ...bootstrapConfig, timeZone: "Europe/Paris", now: () => NOW_MS }}
        dataSource={createMockDataSource({
          ...(options.scenario === undefined ? {} : { scenario: options.scenario }),
          ...(options.onAction === undefined ? {} : { onAction: options.onAction }),
        })}
      >
        <QueryClientProvider
          client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
        >
          <RouterProvider router={router} />
        </QueryClientProvider>
      </AppProvider>
    </ThemeProvider>,
  );
  return { ...view, router };
}
