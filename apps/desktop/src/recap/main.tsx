import "../shared/styles.css";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProvider, bootstrapConfig } from "../shared/app-context";
import { emptyDataSource } from "../shared/data-source";
import { ThemeProvider } from "../shared/theme";
import { RecapApp } from "./recap-app";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}

const queryClient = new QueryClient();

// Phase 11 replaces the config and data source with the daemon's.
createRoot(root).render(
  <StrictMode>
    <ThemeProvider>
      <AppProvider config={bootstrapConfig} dataSource={emptyDataSource}>
        <QueryClientProvider client={queryClient}>
          <RecapApp />
        </QueryClientProvider>
      </AppProvider>
    </ThemeProvider>
  </StrictMode>,
);
