import type { Filters } from "../shared/filters";
import type { Currency } from "../shared/format";
import type { FilterOptions } from "../shared/schemas";
import { THEMES, type Theme } from "../shared/theme";
import { NAV_ITEMS, type NavPath } from "./nav";

/** The filter dimensions the palette can set; each takes one value (`null` clears it). */
export type PaletteDimension = "agent" | "profile" | "project" | "client" | "model";

export type PaletteAction =
  | "scan-now"
  | "export-now"
  | "backup-now"
  | "toggle-currency"
  | { readonly theme: Theme };

/** What choosing an item does (PLAN §7 Cmd-K: go to a screen, apply a filter, run an action). */
export type PaletteCommand =
  | { readonly kind: "go"; readonly to: NavPath }
  | { readonly kind: "filter"; readonly dimension: PaletteDimension; readonly value: string | null }
  | { readonly kind: "action"; readonly action: PaletteAction };

export type PaletteGroup = "Go to" | "Filter" | "Action";

export interface PaletteItem {
  readonly id: string;
  readonly group: PaletteGroup;
  readonly label: string;
  readonly command: PaletteCommand;
}

const DIMENSIONS: readonly {
  readonly dimension: PaletteDimension;
  readonly options: keyof FilterOptions;
}[] = [
  { dimension: "agent", options: "agents" },
  { dimension: "profile", options: "profiles" },
  { dimension: "project", options: "projects" },
  { dimension: "client", options: "clients" },
  { dimension: "model", options: "models" },
];

const THEME_LABELS: Record<Theme, string> = {
  system: "Theme: follow system",
  light: "Theme: light",
  dark: "Theme: dark",
};

function otherCurrency(currency: Currency): Currency {
  return currency === "USD" ? "EUR" : "USD";
}

/** Every command the palette offers for the current filter options and filters. */
export function buildPaletteItems(options: FilterOptions, filters: Filters): PaletteItem[] {
  const go: PaletteItem[] = NAV_ITEMS.map((item) => ({
    id: `go:${item.to}`,
    group: "Go to",
    label: `Go to ${item.label}`,
    command: { kind: "go", to: item.to },
  }));

  const filter: PaletteItem[] = DIMENSIONS.flatMap(({ dimension, options: key }) => [
    ...options[key].map(
      (value): PaletteItem => ({
        id: `filter:${dimension}:${value}`,
        group: "Filter",
        label: `Filter ${dimension}: ${value}`,
        command: { kind: "filter", dimension, value },
      }),
    ),
    ...(filters[dimension] === null
      ? []
      : [
          {
            id: `filter:${dimension}:clear`,
            group: "Filter" as const,
            label: `Clear ${dimension} filter`,
            command: { kind: "filter" as const, dimension, value: null },
          },
        ]),
  ]);

  const actions: readonly { id: string; label: string; action: PaletteAction }[] = [
    { id: "action:scan-now", label: "Scan now", action: "scan-now" },
    { id: "action:export-now", label: "Export now", action: "export-now" },
    { id: "action:backup-now", label: "Back up now", action: "backup-now" },
    {
      id: "action:toggle-currency",
      label: `Show amounts in ${otherCurrency(filters.currency)}`,
      action: "toggle-currency",
    },
    ...THEMES.map((theme) => ({
      id: `action:theme:${theme}`,
      label: THEME_LABELS[theme],
      action: { theme },
    })),
  ];
  const action = actions.map(
    (entry): PaletteItem => ({
      id: entry.id,
      group: "Action",
      label: entry.label,
      command: { kind: "action", action: entry.action },
    }),
  );

  return [...go, ...filter, ...action];
}

/** Every whitespace-separated word of `query` must appear in the label, case-insensitively. */
export function filterItems(items: readonly PaletteItem[], query: string): PaletteItem[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return items.filter((item) => {
    const label = item.label.toLowerCase();
    return words.every((word) => label.includes(word));
  });
}

/** The side effects a command can have; the palette supplies the real ones, tests fakes. */
export interface PaletteEffects {
  readonly filters: Filters;
  readonly navigate: (to: NavPath) => void;
  readonly setFilters: (next: Filters) => void;
  readonly setTheme: (theme: Theme) => void;
  readonly scanNow: () => void;
  readonly exportNow: () => void;
  readonly backupNow: () => void;
}

function runAction(action: PaletteAction, effects: PaletteEffects): void {
  if (typeof action === "object") {
    effects.setTheme(action.theme);
    return;
  }
  switch (action) {
    case "scan-now":
      effects.scanNow();
      return;
    case "export-now":
      effects.exportNow();
      return;
    case "backup-now":
      effects.backupNow();
      return;
    case "toggle-currency":
      effects.setFilters({ ...effects.filters, currency: otherCurrency(effects.filters.currency) });
      return;
    default:
      action satisfies never;
      throw new Error("unreachable palette action");
  }
}

/** Dispatches a palette command to its effect. */
export function runCommand(command: PaletteCommand, effects: PaletteEffects): void {
  switch (command.kind) {
    case "go":
      effects.navigate(command.to);
      return;
    case "filter":
      effects.setFilters({ ...effects.filters, [command.dimension]: command.value });
      return;
    case "action":
      runAction(command.action, effects);
      return;
    default:
      command satisfies never;
      throw new Error("unreachable palette command");
  }
}
