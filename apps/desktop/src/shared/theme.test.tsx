import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { axeViolations } from "./axe";
import { ThemeProvider } from "./theme";
import { ThemeSwitcher } from "./theme-switcher";

function renderSwitcher() {
  return render(
    <ThemeProvider>
      <ThemeSwitcher />
    </ThemeProvider>,
  );
}

const pinned = (): string | undefined => document.documentElement.dataset.theme;

beforeEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ThemeProvider", () => {
  it("starts on System: nothing pinned, so the OS preference applies", () => {
    renderSwitcher();
    expect(pinned()).toBeUndefined();
    expect(screen.getByRole("radio", { name: "System" })).toHaveProperty("checked", true);
  });

  it("pins Dark and Light, persists them, and System unpins", async () => {
    const user = userEvent.setup();
    renderSwitcher();

    await user.click(screen.getByText("Dark"));
    expect(pinned()).toBe("dark");
    expect(localStorage.getItem("theme")).toBe("dark");

    await user.click(screen.getByText("Light"));
    expect(pinned()).toBe("light");

    await user.click(screen.getByText("System"));
    expect(pinned()).toBeUndefined();
    expect(localStorage.getItem("theme")).toBe("system");
  });

  it("restores a stored choice", () => {
    localStorage.setItem("theme", "dark");
    renderSwitcher();
    expect(pinned()).toBe("dark");
  });

  it("ignores a corrupt stored value", () => {
    localStorage.setItem("theme", "neon");
    renderSwitcher();
    expect(pinned()).toBeUndefined();
  });

  it("survives storage errors", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const user = userEvent.setup();
    renderSwitcher();
    await user.click(screen.getByText("Dark"));
    expect(pinned()).toBe("dark");
  });

  it("follows a change made in another window", () => {
    renderSwitcher();
    localStorage.setItem("theme", "light");
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: "theme" }));
    });
    expect(pinned()).toBe("light");
  });

  it("has no axe violations", async () => {
    const { container } = renderSwitcher();
    expect(await axeViolations(container)).toEqual([]);
  });
});
