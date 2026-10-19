import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, type Page, test } from "@playwright/test";

/**
 * Phase 12 screenshots: every full-app screen at 1280 and 390 px, the menu-bar recap at 360 px
 * (All, claude, pi), the special states (empty, offline, incompatible, onboarding), and the quota
 * screen without Claude extra usage, each in light and dark. Output: `e2e/screenshots/<theme>/<name>.png`. Run: `pnpm exec playwright test`.
 */
const OUT = join(import.meta.dirname, "screenshots");
const THEMES = ["light", "dark"] as const;

const SCREENS = [
  "overview",
  "quota",
  "billing",
  "sessions",
  "insights",
  "models",
  "projects",
  "reports",
  "export",
  "settings",
] as const;

const ROUTES: Record<(typeof SCREENS)[number], string> = {
  overview: "/",
  quota: "/quota",
  billing: "/billing",
  sessions: "/sessions",
  insights: "/insights",
  models: "/models",
  projects: "/projects",
  reports: "/reports",
  export: "/export",
  settings: "/settings",
};

const STATES = [
  "empty",
  "error",
  "offline",
  "incompatible",
  "onboarding",
  "partial",
  "stale",
] as const;

const VIEWPORTS = [
  { name: "1280", width: 1280, height: 800 },
  { name: "390", width: 390, height: 844 },
] as const;

async function settle(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle");
  await expect(page.getByText("Loading", { exact: true })).toHaveCount(0);
  // Charts render after the first layout pass.
  await page.waitForTimeout(300);
}

async function shoot(page: Page, theme: string, name: string): Promise<void> {
  const dir = join(OUT, theme);
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const theme of THEMES) {
  test.describe(`${theme} theme`, () => {
    test.use({ colorScheme: theme, reducedMotion: "reduce" });

    for (const viewport of VIEWPORTS) {
      test.describe(`${viewport.name}px app`, () => {
        test.use({ viewport: { width: viewport.width, height: viewport.height } });

        for (const screen of SCREENS) {
          test(`screen ${screen}`, async ({ page }) => {
            await page.goto(`/?mock=populated#${ROUTES[screen]}`);
            await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
            await settle(page);
            await shoot(page, theme, `${viewport.name}-${screen}`);
          });
        }

        for (const state of STATES) {
          test(`state ${state}`, async ({ page }) => {
            await page.goto(`/?mock=${state}#/`);
            if (state === "error") {
              // Reads retry with back-off before they report the failure.
              await expect(page.getByRole("button", { name: "Retry" })).toBeVisible({
                timeout: 20_000,
              });
            }
            await settle(page);
            await shoot(page, theme, `${viewport.name}-state-${state}`);
          });
        }

        test("quota without extra usage", async ({ page }) => {
          await page.goto("/?mock=no-extra-usage#/quota");
          await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
          await settle(page);
          await expect(page.getByRole("meter", { name: "Extra usage" })).toHaveCount(0);
          await shoot(page, theme, `${viewport.name}-quota-no-extra-usage`);
        });

        test("command palette", async ({ page }) => {
          await page.goto("/?mock=populated#/");
          await settle(page);
          await page.getByRole("button", { name: /Search/ }).click();
          await expect(page.getByRole("dialog", { name: "Command palette" })).toBeVisible();
          await shoot(page, theme, `${viewport.name}-palette`);
        });

        test("project drilldown", async ({ page }) => {
          await page.goto("/?mock=populated#/projects/tokenmtr");
          await expect(page.getByRole("heading", { level: 1, name: /tokenmtr/ })).toBeVisible();
          await settle(page);
          await shoot(page, theme, `${viewport.name}-project-detail`);
        });

        test("session drilldown", async ({ page }) => {
          await page.goto("/?mock=populated#/sessions?agent=claude&project=tokenmtr&z=");
          // Hash history appends `?mock=...` to the last hash param, hence the throwaway `z`.
          await page
            .getByRole("region", { name: "Sessions, newest first" })
            .getByRole("link")
            .first()
            .click();
          await expect(page.getByRole("heading", { level: 1, name: /^Session / })).toBeVisible();
          await settle(page);
          await shoot(page, theme, `${viewport.name}-session-detail`);
        });

        test("sessions analysis", async ({ page }) => {
          await page.goto("/?mock=populated#/sessions");
          await page.getByRole("tab", { name: "Analysis" }).click();
          await expect(page.getByRole("figure", { name: "Sessions per day" })).toBeVisible();
          await settle(page);
          await shoot(page, theme, `${viewport.name}-sessions-analysis`);
        });
      });
    }

    test.describe("recap 360px", () => {
      test.use({ viewport: { width: 360, height: 640 } });

      const TABS = [
        { name: "All", file: "All" },
        { name: "Claude Code", file: "claude" },
        { name: "pi", file: "pi" },
      ] as const;
      for (const tab of TABS) {
        test(`tab ${tab.file}`, async ({ page }) => {
          await page.goto("/tray.html?mock=populated");
          await page.getByRole("tab", { name: tab.name }).click();
          await settle(page);
          await shoot(page, theme, `360-recap-${tab.file}`);
        });
      }

      test("footer shows the full health text without clipping", async ({ page }) => {
        await page.goto("/tray.html?mock=populated");
        await settle(page);
        const status = page.getByRole("contentinfo").getByRole("status");
        await expect(status).toHaveText("1 sink failing");
        const clipped = await status.evaluate((element) =>
          [element, ...element.querySelectorAll("*")].some(
            (node) => node.scrollWidth > node.clientWidth,
          ),
        );
        expect(clipped).toBe(false);
        const box = await status.boundingBox();
        expect(box?.x).toBeGreaterThanOrEqual(0);
        expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(360);
      });

      test("empty and offline", async ({ page }) => {
        await page.goto("/tray.html?mock=empty");
        await settle(page);
        await shoot(page, theme, "360-recap-empty");
      });
    });
  });
}
