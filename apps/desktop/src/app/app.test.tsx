import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "../shared/axe";
import { DataSourceError } from "../shared/errors";
import type { MockScenario } from "../shared/mock";
import type { AdvisorReport, QuotaMeter } from "../shared/schemas";
import { renderApp } from "./app-harness";

afterEach(cleanup);

function searchOf(router: { state: { location: { searchStr: string } } }): URLSearchParams {
  return new URLSearchParams(router.state.location.searchStr);
}

/** The first match: `getAll*`/`findAll*` throw when nothing matches, so it always exists. */
function first<T>(items: readonly T[]): T {
  const [item] = items;
  if (item === undefined) {
    throw new Error("no match");
  }
  return item;
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
    await user.click(first(screen.getAllByRole("button", { name: "Keep for 10 years" })));
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
    expect(await screen.findByText("Could not read the usage database")).toBeTruthy();
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
    const top = await screen.findByRole("region", { name: "Top models" });
    await user.click(await within(top).findByRole("button", { name: /claude-opus-4/ }));
    await waitFor(() => expect(searchOf(router).get("model")).toBe("claude-opus-4"));
  });

  it("groups the daily cost by model, then regroups it by agent", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    expect(await screen.findByRole("columnheader", { name: /claude-opus-4/ })).toBeTruthy();
    const group = screen.getByRole("radiogroup", { name: "Group cost" });
    await user.click(within(group).getByRole("radio", { name: "By agent" }));
    await waitFor(() => expect(screen.getByRole("columnheader", { name: /claude/ })).toBeTruthy());
    expect(screen.queryByRole("columnheader", { name: /claude-opus-4/ })).toBeNull();
  });

  it("shows the daily activity calendar by tokens, then by sessions", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    const figure = await screen.findByRole("figure", { name: "Daily activity" });
    expect(within(figure).getByText(/tokens over the last two years/)).toBeTruthy();
    const measure = screen.getByRole("radiogroup", { name: "Activity measure" });
    await user.click(within(measure).getByRole("radio", { name: "Sessions" }));
    expect(within(figure).getByText(/sessions over the last two years/)).toBeTruthy();
  });

  it("moves through the activity calendar with the arrow keys", async () => {
    const user = userEvent.setup();
    await renderApp("/");
    const figure = await screen.findByRole("figure", { name: "Daily activity" });
    const readout = within(figure).getByRole("status");
    expect(readout.textContent).toMatch(/\(today\)/);
    within(figure)
      .getByRole("region", { name: /Daily activity grid/ })
      .focus();
    await user.keyboard("{ArrowLeft}");
    expect(readout.textContent).not.toMatch(/\(today\)/);
    await user.keyboard("{End}");
    expect(readout.textContent).toMatch(/\(today\)/);
  });

  it("keeps the activity calendar when the range has no usage", async () => {
    await renderApp("/?range=custom&from=2024-01-01&to=2024-01-31");
    expect(await screen.findByRole("figure", { name: "Daily activity" })).toBeTruthy();
    expect(screen.queryByRole("figure", { name: "Daily cost" })).toBeNull();
  });

  it("applies the URL filters to the data", async () => {
    await renderApp("/?agent=pi");
    await heading("Overview");
    await screen.findByRole("button", { name: /gpt-5/ });
    expect(screen.queryByRole("button", { name: /claude-opus-4/ })).toBeNull();
  });
});

describe("Quota", () => {
  it("shows every window, per-model ones included, with history and reset markers", async () => {
    await renderApp("/quota");
    expect(await screen.findByRole("figure", { name: "claude / default: 5-hour" })).toBeTruthy();
    expect(screen.getByRole("figure", { name: "claude / default: 7-day" })).toBeTruthy();
    expect(screen.getByRole("figure", { name: "claude / default: Weekly · Fable" })).toBeTruthy();
    expect(screen.getByRole("meter", { name: "Weekly · Fable" })).toBeTruthy();
    expect(screen.getAllByText(/Will not reach 100% before reset/).length).toBeGreaterThan(0);
  });

  it("shows no quota section for an agent without the capability", async () => {
    await renderApp("/quota?agent=pi");
    expect(await screen.findByText("No quota data")).toBeTruthy();
  });

  it("offers to connect the Claude account when the Keychain refused", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/quota", {
      onAction,
      wrap: (ds) => ({
        ...ds,
        quotaLatest: async (filters) =>
          (await ds.quotaLatest(filters)).map(
            (meter): QuotaMeter => ({ ...meter, status: "keychain_denied", windows: [] }),
          ),
      }),
    });
    expect((await screen.findAllByText(/did not let the daemon read/)).length).toBeGreaterThan(0);
    await user.click(first(screen.getAllByRole("button", { name: "Connect Claude account" })));
    await waitFor(() => expect(onAction).toHaveBeenCalledWith("connectKeychain", []));
  });

  it("reloads the credentials of a profile whose token expired", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/quota", {
      onAction,
      wrap: (ds) => ({
        ...ds,
        quotaLatest: async (filters) =>
          (await ds.quotaLatest(filters)).map(
            (meter): QuotaMeter => ({ ...meter, status: "token_expired", windows: [] }),
          ),
      }),
    });
    await user.click(first(await screen.findAllByRole("button", { name: "Refresh" })));
    await waitFor(() => expect(onAction).toHaveBeenCalledWith("connectKeychain", []));
  });
});

describe("Billing", () => {
  it("tracks a detected plan through the subscription form", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/billing", {
      onAction,
      wrap: (ds) => ({
        ...ds,
        advisor: async (filters) =>
          (await ds.advisor(filters)).map(
            (report): AdvisorReport => ({ ...report, current_plan: "team" }),
          ),
      }),
    });
    await user.click(await screen.findByRole("button", { name: /^Track it: claude/ }));
    const dialog = await screen.findByRole("dialog", { name: "Track subscription" });
    // The fee is required: an empty form adds nothing.
    await user.click(within(dialog).getByRole("button", { name: "Add subscription" }));
    expect(onAction).not.toHaveBeenCalledWith("addSubscription", expect.anything());
    await user.type(within(dialog).getByRole("textbox", { name: /Fee per month/ }), "30");
    await user.click(within(dialog).getByRole("button", { name: "Add subscription" }));
    await waitFor(() =>
      expect(onAction).toHaveBeenCalledWith("addSubscription", [
        expect.objectContaining({
          id: "claude-default-team",
          provider: "anthropic",
          plan: "team",
          agents: ["claude"],
          profiles: ["default"],
          fee: { usd: 30_000_000 },
          period: "monthly",
          start: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
          end: null,
        }),
      ]),
    );
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
    const link = first(await within(table).findAllByRole("link"));
    const id = link.textContent ?? "";
    await user.click(link);
    expect(await screen.findByRole("heading", { level: 1, name: /^Session / })).toBeTruthy();
    expect(id).not.toBe("");
    const resume = await screen.findByText(/claude --resume/);
    expect(resume.textContent).toMatch(/^cd \/Users\/dev\/src\/\S+ && claude --resume /);
    await user.click(screen.getByRole("button", { name: "Copy command" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeTruthy();
    expect(screen.getByText("Resume command copied").getAttribute("role")).toBe("status");
    expect(await navigator.clipboard.readText()).toBe(resume.textContent);
    expect(screen.getByRole("region", { name: "Session events" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reveal transcript" })).toBeTruthy();
  });

  it("builds the pi resume command", async () => {
    const user = userEvent.setup();
    await renderApp("/sessions?agent=pi&project=acme-web");
    const table = await screen.findByRole("region", { name: "Sessions, newest first" });
    await user.click(first(await within(table).findAllByRole("link")));
    const resume = await screen.findByText(/pi --session/);
    expect(resume.textContent).toMatch(/^cd \/Users\/dev\/src\/\S+ && pi --session /);
  });

  it("goes back to the list keeping the filters", async () => {
    const user = userEvent.setup();
    const { router } = await renderApp("/sessions?agent=claude");
    const table = await screen.findByRole("region", { name: "Sessions, newest first" });
    await user.click(first(await within(table).findAllByRole("link")));
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
    const { router } = await renderApp("/projects?agent=claude");
    expect(await screen.findByRole("heading", { name: "Acme" })).toBeTruthy();
    await user.click(await screen.findByRole("button", { name: /tokenmtr/ }));
    const branches = await screen.findByRole("list", { name: "Branches by API value" });
    expect(within(branches).getByText("main")).toBeTruthy();
    // The drilldown is a route: linkable, and Back returns to the list with the filters.
    expect(router.state.location.pathname).toBe("/projects/tokenmtr");
    expect(searchOf(router).get("agent")).toBe("claude");
    await user.click(screen.getByRole("button", { name: "All projects" }));
    await heading("Projects");
    expect(searchOf(router).get("agent")).toBe("claude");
  });

  it("adds a rule for an unresolved directory", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/projects", { onAction });
    await user.click(
      await screen.findByRole("button", { name: "Add rule for /Users/dev/scratch/experiments" }),
    );
    // Prefilled with the suggested name, which the user may change.
    const name = await screen.findByRole("textbox", { name: "Project name" });
    expect(name).toHaveProperty("value", "experiments");
    await user.clear(name);
    await user.type(name, "Lab");
    await user.click(screen.getByRole("button", { name: "Add rule" }));
    await waitFor(() =>
      expect(onAction).toHaveBeenCalledWith("addProjectRule", [
        "/Users/dev/scratch/experiments",
        "Lab",
      ]),
    );
  });

  it("does not add a rule without a name", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/projects", { onAction });
    await user.click(
      await screen.findByRole("button", { name: "Add rule for /Users/dev/scratch/experiments" }),
    );
    await user.clear(await screen.findByRole("textbox", { name: "Project name" }));
    await user.click(screen.getByRole("button", { name: "Add rule" }));
    expect(onAction).not.toHaveBeenCalledWith("addProjectRule", expect.anything());
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
    await user.click(screen.getByRole("button", { name: "Edit config file" }));
    await waitFor(() => {
      expect(onAction).toHaveBeenCalledWith("scanNow", []);
      expect(onAction).toHaveBeenCalledWith("backupNow", []);
      expect(onAction).toHaveBeenCalledWith("revealPath", [{ kind: "exports_dir" }]);
      expect(onAction).toHaveBeenCalledWith("editConfig", []);
    });
  });

  it("tells to move the app to Applications when the login item is refused", async () => {
    const user = userEvent.setup();
    await renderApp("/settings", {
      onAction: (action) => {
        if (action === "setLaunchAtLogin") {
          throw new DataSourceError("set_launch_at_login", {
            kind: "app_not_installed",
            message: "/Volumes/TokenMtr/TokenMtr.app runs from a disk image",
          });
        }
      },
    });
    await user.click(await screen.findByRole("switch", { name: "Launch at login" }));
    expect(
      await screen.findByText("Move TokenMtr to Applications, then open it from there."),
    ).toBeTruthy();
  });

  it("names the tray and drops the macOS-only cards on Linux", async () => {
    await renderApp("/settings", { os: "linux" });
    expect(await screen.findByRole("heading", { name: "Tray" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Menu bar" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Protected folders" })).toBeNull();
    cleanup();
    await renderApp("/settings", { os: "macos" });
    expect(await screen.findByRole("heading", { name: "Menu bar" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Protected folders" })).toBeTruthy();
  });

  it("shows the Ctrl+K hint on Linux", async () => {
    await renderApp("/", { os: "linux" });
    expect(await screen.findByText("Ctrl+K")).toBeTruthy();
    cleanup();
    await renderApp("/", { os: "macos" });
    expect(await screen.findByText("⌘K")).toBeTruthy();
  });

  it("chooses the menu-bar rings and the profiles drawn without rings", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/settings", { onAction });
    const perModel = await screen.findByRole("switch", { name: /Per model/ });
    expect(screen.getByRole("switch", { name: /Session/ })).toHaveProperty("checked", true);
    expect(perModel).toHaveProperty("checked", false);
    await user.click(perModel);
    await waitFor(() =>
      expect(onAction).toHaveBeenCalledWith("setMenubarRings", [
        ["session", "weekly", "per_model"],
        [],
      ]),
    );
    await user.click(await screen.findByRole("switch", { name: "work" }));
    await waitFor(() =>
      expect(onAction).toHaveBeenCalledWith("setMenubarRings", [
        ["session", "weekly", "per_model"],
        ["work"],
      ]),
    );
  });

  it("sets the default remote and branch, rejecting a blank remote", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/settings", { onAction });
    const remote = await screen.findByRole("textbox", { name: "Default remote" });
    expect(remote).toHaveProperty("value", "origin");
    const save = within(screen.getByRole("region", { name: "Git" })).getByRole("button", {
      name: "Save",
    });
    await user.clear(remote);
    await user.click(save);
    expect(onAction).not.toHaveBeenCalledWith("setGitDefaults", expect.anything());
    await user.type(remote, " upstream ");
    await user.type(screen.getByRole("textbox", { name: "Default branch" }), "main");
    await user.click(save);
    await waitFor(() =>
      expect(onAction).toHaveBeenCalledWith("setGitDefaults", ["upstream", "main"]),
    );
  });

  it("offers 10-year retention per profile with a retention window", async () => {
    const onAction = vi.fn();
    const user = userEvent.setup();
    await renderApp("/settings", { onAction });
    await user.click(first(await screen.findAllByRole("button", { name: "Keep for 10 years" })));
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
