import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Two entries: `index.html` is the full app, `tray.html` is the menu-bar recap.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: "safari16",
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, "index.html"),
        tray: resolve(import.meta.dirname, "tray.html"),
      },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["src/test-setup.ts"],
  },
});
