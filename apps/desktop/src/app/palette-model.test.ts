import { describe, expect, it, vi } from "vitest";
import { defaultFilters } from "../shared/filters";
import type { FilterOptions } from "../shared/schemas";
import { buildPaletteItems, filterItems, type PaletteEffects, runCommand } from "./palette-model";

const OPTIONS: FilterOptions = {
  agents: ["claude", "pi"],
  profiles: ["default", "work"],
  models: ["opus-4"],
  projects: ["acme-web", "tokenmtr"],
  clients: ["Acme"],
};

const FILTERS = defaultFilters("USD");

function labels(query: string, filters = FILTERS): string[] {
  return filterItems(buildPaletteItems(OPTIONS, filters), query).map((item) => item.label);
}

function effects(overrides: Partial<PaletteEffects> = {}): PaletteEffects {
  return {
    filters: FILTERS,
    navigate: vi.fn(),
    setFilters: vi.fn(),
    setTheme: vi.fn(),
    scanNow: vi.fn(),
    exportNow: vi.fn(),
    backupNow: vi.fn(),
    ...overrides,
  };
}

function command(id: string) {
  const item = buildPaletteItems(OPTIONS, FILTERS).find((candidate) => candidate.id === id);
  if (item === undefined) {
    throw new Error(`no palette item ${id}`);
  }
  return item.command;
}

describe("palette items", () => {
  it("offers a screen, a filter per option and the actions", () => {
    const all = labels("");
    expect(all).toContain("Go to Billing");
    expect(all).toContain("Filter agent: claude");
    expect(all).toContain("Filter project: acme-web");
    expect(all).toContain("Filter client: Acme");
    expect(all).toContain("Scan now");
    expect(all).toContain("Export now");
    expect(all).toContain("Back up now");
    expect(all).toContain("Show amounts in EUR");
    expect(all).toContain("Theme: dark");
  });

  it("matches every word, case-insensitively", () => {
    expect(labels("FILTER acme")).toEqual(["Filter project: acme-web", "Filter client: Acme"]);
    expect(labels("zzz")).toEqual([]);
  });

  it("offers to clear a filter only once it is set, and the other currency", () => {
    expect(labels("clear")).toEqual([]);
    const filtered = { ...FILTERS, agent: "pi", currency: "EUR" } as const;
    expect(labels("clear", filtered)).toEqual(["Clear agent filter"]);
    expect(labels("amounts", filtered)).toEqual(["Show amounts in USD"]);
  });
});

describe("palette dispatch", () => {
  it("navigates to a screen", () => {
    const fx = effects();
    runCommand(command("go:/billing"), fx);
    expect(fx.navigate).toHaveBeenCalledWith("/billing");
  });

  it("applies and clears a filter, keeping the others", () => {
    const fx = effects({ filters: { ...FILTERS, model: "opus-4" } });
    runCommand(command("filter:agent:claude"), fx);
    expect(fx.setFilters).toHaveBeenCalledWith({ ...FILTERS, model: "opus-4", agent: "claude" });
  });

  it("runs actions", () => {
    const fx = effects();
    runCommand(command("action:scan-now"), fx);
    runCommand(command("action:export-now"), fx);
    runCommand(command("action:backup-now"), fx);
    runCommand(command("action:theme:dark"), fx);
    runCommand(command("action:toggle-currency"), fx);
    expect(fx.scanNow).toHaveBeenCalledOnce();
    expect(fx.exportNow).toHaveBeenCalledOnce();
    expect(fx.backupNow).toHaveBeenCalledOnce();
    expect(fx.setTheme).toHaveBeenCalledWith("dark");
    expect(fx.setFilters).toHaveBeenCalledWith({ ...FILTERS, currency: "EUR" });
  });
});
