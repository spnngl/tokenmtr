import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DataSourceError } from "../shared/errors";
import { renderApp } from "./app-harness";

afterEach(cleanup);

/** A failure the way the Tauri data source reports a `store` `CommandError`. */
function locked(command: string): Promise<never> {
  return Promise.reject(
    new DataSourceError(command, { kind: "store", message: "database is locked" }),
  );
}

async function heading(name: string): Promise<HTMLElement> {
  return screen.findByRole("heading", { level: 1, name });
}

function toasts(): HTMLElement {
  return screen.getByRole("region", { name: /Notifications/ });
}

describe("currency of money figures (H1)", () => {
  // EUR asked, no FX rate yet: the backend answers in USD and every figure must say so.
  const SCREENS = [
    { url: "/?currency=EUR", title: "Overview" },
    { url: "/models?currency=EUR", title: "Models" },
    { url: "/projects?currency=EUR", title: "Projects" },
    { url: "/insights?currency=EUR", title: "Insights" },
    { url: "/sessions?currency=EUR", title: "Sessions" },
  ] as const;

  it.each(SCREENS)("$title labels amounts in USD when EUR has no rate", async ({ url, title }) => {
    const { container } = await renderApp(url, { noFxRates: true });
    await heading(title);
    await waitFor(() => expect(container.textContent).toMatch(/\$\d/));
    expect(container.textContent).not.toMatch(/€/);
  });

  it("explains the fallback on the overview", async () => {
    await renderApp("/?currency=EUR", { noFxRates: true });
    expect(await screen.findByText("EUR rates unavailable: shown in USD")).toBeTruthy();
  });

  it("labels amounts in EUR when a rate exists", async () => {
    const { container } = await renderApp("/?currency=EUR");
    await heading("Overview");
    await waitFor(() => expect(container.textContent).toMatch(/€\d/));
    expect(screen.queryByText("EUR rates unavailable: shown in USD")).toBeNull();
  });
});

describe("a failed read stays an error (H5)", () => {
  it("keeps the rest of Settings and shows the parse-issue error with a retry", async () => {
    await renderApp("/settings", { wrap: (ds) => ({ ...ds, parseIssues: () => locked("x") }) });
    await heading("Settings");
    const card = await screen.findByRole("region", { name: "Parse issues" });
    expect(await within(card).findByText("Could not read the usage database")).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Retry" })).toBeTruthy();
    expect(screen.queryByText(/No parse issue/)).toBeNull();
    expect(screen.getByRole("region", { name: "General" })).toBeTruthy();
  });

  it("keeps the overview KPIs when only the daily series fails", async () => {
    await renderApp("/", { wrap: (ds) => ({ ...ds, series: () => locked("series") }) });
    await heading("Overview");
    const card = await screen.findByRole("region", { name: "Daily cost" });
    expect(await within(card).findByText(/database is locked/)).toBeTruthy();
    expect(screen.getByText("Cached input")).toBeTruthy();
  });
});

describe("action feedback (H6)", () => {
  it("announces a failed action with its reason", async () => {
    const user = userEvent.setup();
    await renderApp("/settings", { wrap: (ds) => ({ ...ds, scanNow: () => locked("scan_now") }) });
    await user.click(await screen.findByRole("button", { name: "Scan now" }));
    expect(await within(toasts()).findByText("Scan failed")).toBeTruthy();
    expect(within(toasts()).getByText("database is locked")).toBeTruthy();
  });

  it("confirms a finished backup", async () => {
    const user = userEvent.setup();
    await renderApp("/settings");
    await user.click(await screen.findByRole("button", { name: "Back up now" }));
    expect(await within(toasts()).findByText("Backup saved")).toBeTruthy();
  });

  it("reports a palette action after the palette closed", async () => {
    const user = userEvent.setup();
    await renderApp("/", { wrap: (ds) => ({ ...ds, scanNow: () => locked("scan_now") }) });
    await heading("Overview");
    await user.keyboard("{Control>}k{/Control}");
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    await user.click(within(dialog).getByRole("menuitem", { name: "Scan now" }));
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
    expect(await within(toasts()).findByText("Scan failed")).toBeTruthy();
  });

  it("erases after the phrase, then clears it and confirms", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/settings", { onAction });
    const phrase = await screen.findByRole("textbox", { name: /to confirm/ });
    await user.type(phrase, "erase all data");
    await user.click(screen.getByRole("button", { name: "Erase all data" }));
    expect(await within(toasts()).findByText("All data erased")).toBeTruthy();
    expect(onAction).toHaveBeenCalledWith("eraseAllData", []);
    expect(screen.getByRole("textbox", { name: /to confirm/ })).toHaveProperty("value", "");
  });
});

describe("command palette focus", () => {
  it("returns focus to the trigger when closed", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    const trigger = await screen.findByRole("button", { name: /Search/ });
    await user.click(trigger);
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeTruthy();
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull(),
    );
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });

  it("toggles with the shortcut", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    await heading("Overview");
    await user.keyboard("{Control>}k{/Control}");
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeTruthy();
    await user.keyboard("{Control>}k{/Control}");
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull(),
    );
  });
});

describe("skip link", () => {
  it("moves focus to the routed screen", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    await heading("Overview");
    await user.click(screen.getByRole("button", { name: "Skip to content" }));
    expect(document.activeElement?.tagName).toBe("MAIN");
  });
});
