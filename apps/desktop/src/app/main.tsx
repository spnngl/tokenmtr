import "../shared/styles.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}

createRoot(root).render(
  <StrictMode>
    <main className="p-6">
      <h1 className="text-xl font-semibold">TokenMtr</h1>
    </main>
  </StrictMode>,
);
