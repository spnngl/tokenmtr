import "../shared/styles.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("missing #root");
}

createRoot(root).render(
  <StrictMode>
    <main className="p-4">
      <h1 className="text-base font-semibold">TokenMtr recap</h1>
    </main>
  </StrictMode>,
);
