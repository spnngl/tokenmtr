import "../shared/styles.css";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createHashHistory, RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProvider } from "../shared/app-provider";
import { navigationSearch, subscribeToShell } from "../shared/events";
import { createRuntime } from "../shared/runtime";
import { ThemeProvider } from "../shared/theme";
import { createAppRouter } from "./router";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}

const queryClient = new QueryClient();
// Hash history: the webview loads `index.html`, so path routes would not match (D29 deep links
// pass `#/route?filters`).
const history = createHashHistory();
const router = createAppRouter(history);

const runtime = createRuntime(window.location.search);
if (runtime.listen !== undefined) {
  void subscribeToShell({
    listen: runtime.listen,
    queryClient,
    window: "main",
    navigate: ({ route, agent }) => {
      void router.navigate({
        to: route,
        search: (current: Record<string, unknown>) => navigationSearch(current, agent),
      });
    },
  });
}

createRoot(root).render(
  <StrictMode>
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <AppProvider config={runtime.config} dataSource={runtime.dataSource}>
          <RouterProvider router={router} />
        </AppProvider>
      </QueryClientProvider>
    </ThemeProvider>
  </StrictMode>,
);
