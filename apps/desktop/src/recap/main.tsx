import "../shared/styles.css";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProvider } from "../shared/app-provider";
import { subscribeToShell } from "../shared/events";
import { createRuntime } from "../shared/runtime";
import { ThemeProvider } from "../shared/theme";
import { RecapApp } from "./recap-app";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}

const queryClient = new QueryClient();
const runtime = createRuntime(window.location.search);
if (runtime.listen !== undefined) {
  void subscribeToShell({ listen: runtime.listen, queryClient, window: "recap" });
}

createRoot(root).render(
  <StrictMode>
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <AppProvider config={runtime.config} dataSource={runtime.dataSource}>
          <RecapApp />
        </AppProvider>
      </QueryClientProvider>
    </ThemeProvider>
  </StrictMode>,
);
