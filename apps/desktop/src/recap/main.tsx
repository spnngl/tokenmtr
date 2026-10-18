import "../shared/styles.css";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProvider } from "../shared/app-context";
import { createRuntime } from "../shared/runtime";
import { ThemeProvider } from "../shared/theme";
import { RecapApp } from "./recap-app";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}

const queryClient = new QueryClient();
const runtime = createRuntime(window.location.search);

createRoot(root).render(
  <StrictMode>
    <ThemeProvider>
      <AppProvider config={runtime.config} dataSource={runtime.dataSource}>
        <QueryClientProvider client={queryClient}>
          <RecapApp />
        </QueryClientProvider>
      </AppProvider>
    </ThemeProvider>
  </StrictMode>,
);
