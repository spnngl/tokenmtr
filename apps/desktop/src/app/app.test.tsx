import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../shared/axe";
import type { MockScenario } from "../shared/mock";
import { renderApp } from "./app-harness";

afterEach(cleanup);

function searchOf(router: { state: { location: { searchStr: string } } }): URLSearchParams {
  return new URLSearchParams(router.state.location.searchStr);
}

async function heading(name: string): Promise<HTMLElement> {
  return screen.findByRole("heading", { level: 1, name });
}

describe("app gate", () => {
  it("shows the daemon-offline state with an install action", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/", { scenario: "offline", onAction });
    expect(await screen.findByText("Daemon not running")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Install / start daemon" }));
    await waitFor(() => expect(onAction).toHaveBeenCalledWith("installDaemon", []));
  });

  it("shows the incompatible state with both versions and a restart action", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/", { scenario: "incompatible", onAction });
    expect(await screen.findByText("Update required")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Restart daemon" }));
    await waitFor(() => expect(onAction).toHaveBeenCalledWith("restartDaemon", []));
  });

  it("walks through onboarding and then opens the dashboard", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/", { scenario: "onboarding", onAction });
    expect(await screen.findByRole("heading", { name: "Welcome to TokenMtr" })).toBeTruthy();
    expect(screen.getByText("Detected agents")).toBeTruthy();
    await user.click(
      screen.getAllByRole("button", { name: "Keep for 10 years" })[0] as HTMLElement,
    );
    await user.click(screen.getByRole("button", { name: "Install daemon" }));
    await user.click(screen.getByRole("button", { name: "Allow notifications" }));
    await waitFor(() => {
      expect(onAction).toHaveBeenCalledWith("keepTenYears", ["claude", "default"]);
      expect(onAction).toHaveBeenCalledWith("installDaemon", []);
    });
    await user.click(screen.getByRole("button", { name: "Open dashboard" }));
    expect(await heading("Overview")).toBeTruthy();
  });

  it("has no axe violations during onboarding", async () => {
    const { container } = await renderApp("/", { scenario: "onboarding" });
    await screen.findByRole("heading", { name: "Welcome to TokenMtr" });
    expect(await axeViolations(container)).toEqual([]);
  });

  it("warns when the last scan is stale and offers a scan", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/", { scenario: "stale", onAction });
    expect(await screen.findByText(/Last scan .* ago/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Scan now" }));
    await waitFor(() => expect(onAction).toHaveBeenCalledWith("scanNow", []));
  });

  it("warns about failed collectors with a link to the parse issues", async () => {
    await renderApp("/", { scenario: "partial" });
    expect(await screen.findByText(/1 collector failed/)).toBeTruthy();
    expect(screen.getByRole("link", { name: "View parse issues" })).toBeTruthy();
  });
});

const SCREENS = [
  { url: "/", title: "Overview" },
  { url: "/quota", title: "Quota" },
  { url: "/sessions", title: "Sessions" },
  { url: "/projects", title: "Projects" },
  { url: "/settings", title: "Settings" },
] as const;

describe.each(SCREENS)("$title screen states", ({ url, title }) => {
  it("is accessible once populated", async () => {
    const { container } = await renderApp(url);
    await heading(title);
    await waitFor(() => expect(screen.queryByText("Loading")).toBeNull());
    expect(await axeViolations(container)).toEqual([]);
  });

  it("shows a loading state while reading", async () => {
    await renderApp(url, { scenario: "loading" });
    await heading(title);
    expect(await screen.findByText("Loading")).toBeTruthy();
  });

  it("shows an error with a retry", async () => {
    await renderApp(url, { scenario: "error" });
    await heading(title);
    expect(await screen.findByText("Could not load data")).toBeTruthy();
    expect(screen.getByText(/database is locked/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });
});

describe.each(SCREENS.filter((screen) => screen.url !== "/settings"))(
  "$title screen with no data",
  ({ url, title }) => {
    it("explains the empty state and stays accessible", async () => {
      const { container } = await renderApp(url, { scenario: "empty" });
      await heading(title);
      await screen.findByText(/No (usage in this range|quota data|sessions)/);
      expect(await axeViolations(container)).toEqual([]);
    });
  },
);

describe.each(SCREENS)("$title screen with the daemon offline", ({ url }) => {
  it("replaces the screen with the offline state", async () => {
    await renderApp(url, { scenario: "offline" });
    expect(await screen.findByText("Daemon not running")).toBeTruthy();
  });
});

describe("Overview", () => {
  it("shows KPIs, the quota ETA and applies a filter from a top model", async () => {
    const user = userEvent.setup();
    const { router } = await renderApp("/");
    await heading("Overview");
    expect(await screen.findByText(/100% at 15:40 at this pace/)).toBeTruthy();
    expect(screen.getByText("Cached input")).toBeTruthy();
    expect(screen.getByText("Cache savings")).toBeTruthy();
    await user.click(await screen.findByRole("button", { name: /claude-opus-4/ }));
    await waitFor(() => expect(searchOf(router).get("model")).toBe("claude-opus-4"));
  });

  it("regroups the daily cost by model", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    const group = await screen.findByRole("radiogroup", { name: "Group cost" });
    await user.click(within(group).getByRole("radio", { name: "By model" }));
    await waitFor(() =>
      expect(screen.getByRole("columnheader", { name: /claude-opus-4/ })).toBeTruthy(),
    );
  });

  it("applies the URL filters to the data", async () => {
    await renderApp("/?agent=pi");
    await heading("Overview");
    await screen.findByRole("button", { name: /gpt-5/ });
    expect(screen.queryByRole("button", { name: /claude-opus-4/ })).toBeNull();
  });
});

describe("Quota", () => {
  it("shows both windows with their history and reset markers", async () => {
    await renderApp("/quota");
    expect(
      await screen.findByRole("figure", { name: "claude / default: 5-hour window" }),
    ).toBeTruthy();
    expect(screen.getByRole("figure", { name: "claude / default: 7-day window" })).toBeTruthy();
    expect(screen.getByText(/Will not reach 100% before reset/)).toBeTruthy();
  });

  it("shows no quota section for an agent without the capability", async () => {
    await renderApp("/quota?agent=pi");
    expect(await screen.findByText("No quota data")).toBeTruthy();
  });
});

describe("Sessions", () => {
  it("pages through sessions", async () => {
    const user = userEvent.setup();
    await renderApp("/sessions");
    const table = await screen.findByRole("region", { name: "Sessions, newest first" });
    const first = (await within(table).findAllByRole("link"))[0]?.textContent;
    await user.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => expect(within(table).getAllByRole("link")[0]?.textContent).not.toBe(first));
    expect(screen.getByRole("button", { name: "Previous" })).toBeTruthy();
  });

  it("opens a claude session with a copyable resume command", async () => {
    const user = userEvent.setup();
    await renderApp("/sessions?agent=claude&project=tokenmtr");
    const table = await screen.findByRole("region", { name: "Sessions, newest first" });
    const link = (await within(table).findAllByRole("link"))[0] as HTMLElement;
    const id = link.textContent ?? "";
    await user.click(link);
    expect(await screen.findByRole("heading", { level: 1, name: /^Session / })).toBeTruthy();
    expect(id).not.toBe("");
    const resume = await screen.findByText(/claude --resume/);
    expect(resume.textContent).toMatch(/^cd \/Users\/dev\/src\/\S+ && claude --resume /);
    await user.click(screen.getByRole("button", { name: "Copy command" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeTruthy();
    expect(await navigator.clipboard.readText()).toBe(resume.textContent);
    expect(screen.getByRole("region", { name: "Session events" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reveal transcript" })).toBeTruthy();
  });

  it("builds the pi resume command", async () => {
    const user = userEvent.setup();
    await renderApp("/sessions?agent=pi&project=acme-web");
    const table = await screen.findByRole("region", { name: "Sessions, newest first" });
    await user.click((await within(table).findAllByRole("link"))[0] as HTMLElement);
    const resume = await screen.findByText(/pi --session/);
    expect(resume.textContent).toMatch(/^cd \/Users\/dev\/src\/\S+ && pi --session /);
  });

  it("goes back to the list keeping the filters", async () => {
    const user = userEvent.setup();
    const { router } = await renderApp("/sessions?agent=claude");
    const table = await screen.findByRole("region", { name: "Sessions, newest first" });
    await user.click((await within(table).findAllByRole("link"))[0] as HTMLElement);
    await user.click(await screen.findByRole("link", { name: /All sessions/ }));
    await heading("Sessions");
    expect(searchOf(router).get("agent")).toBe("claude");
  });

  it("explains a session that does not exist", async () => {
    await renderApp("/sessions/claude/does-not-exist");
    expect(await screen.findByText("Session not found")).toBeTruthy();
  });
});

describe("Projects", () => {
  it("groups projects by client and drills into branches", async () => {
    const user = userEvent.setup();
    await renderApp("/projects");
    expect(await screen.findByRole("heading", { name: "Acme" })).toBeTruthy();
    await user.click(await screen.findByRole("button", { name: /tokenmtr/ }));
    expect(await screen.findByText("main")).toBeTruthy();
  });

  it("adds a rule for an unresolved directory", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/projects", { onAction });
    await user.click(
      await screen.findByRole("button", { name: "Add rule for /Users/dev/scratch/experiments" }),
    );
    await waitFor(() =>
      expect(onAction).toHaveBeenCalledWith("addProjectRule", ["/Users/dev/scratch/experiments"]),
    );
  });
});

describe("Settings", () => {
  const cases: readonly { scenario: MockScenario; text: RegExp }[] = [
    { scenario: "populated", text: /changed its log format/ },
    { scenario: "populated", text: /TokenMtr cannot read these until you allow it/ },
  ];
  it.each(cases)("shows $text", async ({ scenario, text }) => {
    await renderApp("/settings", { scenario });
    expect(await screen.findByText(text)).toBeTruthy();
  });

  it("runs the actions", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/settings", { onAction });
    await user.click(await screen.findByRole("button", { name: "Scan now" }));
    await user.click(screen.getByRole("button", { name: "Back up now" }));
    await user.click(screen.getByRole("button", { name: "Reveal exports folder" }));
    await waitFor(() => {
      expect(onAction).toHaveBeenCalledWith("scanNow", []);
      expect(onAction).toHaveBeenCalledWith("backupNow", []);
      expect(onAction).toHaveBeenCalledWith("revealPath", [expect.stringMatching(/TokenMtr/)]);
    });
  });

  it("offers 10-year retention per profile with a retention window", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/settings", { onAction });
    await user.click(
      (await screen.findAllByRole("button", { name: "Keep for 10 years" }))[0] as HTMLElement,
    );
    await waitFor(() =>
      expect(onAction).toHaveBeenCalledWith("keepTenYears", ["claude", "default"]),
    );
  });

  it("keeps erasing disabled until the phrase is typed", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/settings", { onAction });
    const erase = await screen.findByRole("button", { name: "Erase all data" });
    expect(erase.hasAttribute("disabled") || erase.getAttribute("aria-disabled") === "true").toBe(
      true,
    );
    await user.type(screen.getByRole("textbox", { name: /to confirm/ }), "erase all data");
    await user.click(screen.getByRole("button", { name: "Erase all data" }));
    await waitFor(() => expect(onAction).toHaveBeenCalledWith("eraseAllData", []));
  });
});

describe("Command palette", () => {
  async function openPalette(url = "/") {
    const user = userEvent.setup();
    const view = await renderApp(url);
    await heading("Overview");
    await user.keyboard("{Control>}k{/Control}");
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    return { user, dialog, ...view };
  }

  it("opens with Ctrl+K and filters as you type", async () => {
    const { user, dialog } = await openPalette();
    expect(within(dialog).getAllByRole("menuitem").length).toBeGreaterThan(10);
    await user.type(within(dialog).getByRole("searchbox", { name: "Search commands" }), "bill");
    const items = within(dialog).getAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual(["Go to Billing"]);
  });

  it("shows an empty state", async () => {
    const { user, dialog } = await openPalette();
    await user.type(within(dialog).getByRole("searchbox", { name: "Search commands" }), "zzzz");
    expect(within(dialog).getByText("No matching command.")).toBeTruthy();
  });

  it("navigates to a screen on Enter", async () => {
    const { user, dialog, router } = await openPalette();
    await user.type(
      within(dialog).getByRole("searchbox", { name: "Search commands" }),
      "go to quota",
    );
    await user.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe("/quota"));
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
  });

  it("applies a filter", async () => {
    const { user, dialog, router } = await openPalette();
    await user.click(within(dialog).getByRole("menuitem", { name: "Filter agent: pi" }));
    await waitFor(() => expect(searchOf(router).get("agent")).toBe("pi"));
  });

  it("toggles the currency", async () => {
    const { user, dialog, router } = await openPalette();
    await user.click(within(dialog).getByRole("menuitem", { name: "Show amounts in EUR" }));
    await waitFor(() => expect(searchOf(router).get("currency")).toBe("EUR"));
  });

  it("opens from the header button", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    await user.click(await screen.findByRole("button", { name: /Search/ }));
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeTruthy();
  });

  it("is accessible", async () => {
    const { dialog } = await openPalette();
    expect(await axeViolations(dialog)).toEqual([]);
  });
});
