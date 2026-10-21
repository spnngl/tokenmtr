import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { bootstrapConfig } from "./app-context";
import { AppProvider } from "./app-provider";
import { createMockDataSource } from "./mock";

/**
 * Test helper: a `render` wrapper giving components that run data actions (such as the quota
 * Refresh button) a query client, the mock data source and the toast region.
 */
export function TestProviders(props: { readonly children: ReactNode }): ReactNode {
  return (
    <QueryClientProvider client={new QueryClient()}>
      <AppProvider config={bootstrapConfig} dataSource={createMockDataSource()}>
        {props.children}
      </AppProvider>
    </QueryClientProvider>
  );
}
