import "../shared/styles.css";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createHashHistory, RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProvider, bootstrapConfig } from "../shared/app-context";
import { emptyDataSource } from "../shared/data-source";
import { ThemeProvider } from "../shared/theme";
import { createAppRouter } from "./router";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}

const queryClient = new QueryClient();
// Hash history: the webview loads `index.html`, so path routes would not match (D29 deep links
// pass `#/route?filters`). Phase 11 replaces these values with the daemon's config.
const router = createAppRouter(createHashHistory());

createRoot(root).render(
  <StrictMode>
    <ThemeProvider>
      <AppProvider config={bootstrapConfig} dataSource={emptyDataSource}>
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </AppProvider>
    </ThemeProvider>
  </StrictMode>,
);
