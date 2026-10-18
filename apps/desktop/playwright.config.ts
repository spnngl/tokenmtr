import { defineConfig } from "@playwright/test";

/**
 * Screenshots run against `vite dev` with the seeded mock data source (no Tauri). They use the
 * system Chrome (`channel: "chrome"`) so no browser download is needed; set `PW_CHANNEL=` to use
 * Playwright's own Chromium after `pnpm exec playwright install chromium`.
 */
const channel = process.env.PW_CHANNEL ?? "chrome";

export default defineConfig({
  testDir: "e2e",
  outputDir: "e2e/results",
  fullyParallel: true,
  reporter: "list",
  use: {
    baseURL: "http://localhost:1420",
    ...(channel === "" ? {} : { channel }),
  },
  webServer: {
    command: "pnpm dev",
    url: "http://localhost:1420",
    reuseExistingServer: true,
  },
});
