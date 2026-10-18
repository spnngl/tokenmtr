import "../shared/styles.css";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createHashHistory, RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProvider } from "../shared/app-context";
import { subscribeToShell } from "../shared/events";
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

const runtime = await createRuntime(window.location.search);
if (runtime.listen !== undefined) {
  await subscribeToShell({
    listen: runtime.listen,
    queryClient,
    window: "main",
    navigate: (path) => history.push(path),
  });
}

createRoot(root).render(
  <StrictMode>
    <ThemeProvider>
      <AppProvider config={runtime.config} dataSource={runtime.dataSource}>
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </AppProvider>
    </ThemeProvider>
  </StrictMode>,
);
