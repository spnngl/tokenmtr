import type { ReactNode } from "react";
import { StateView } from "../shared/state-view";
import { ThemeSwitcher } from "../shared/theme-switcher";
import type { NavItem } from "./nav";

/**
 * A screen with its title and the state it is in. Phase 12 step 3 replaces each body, one screen
 * at a time; until the data layer exists every screen is honestly empty.
 */
export function PlaceholderPage({ item }: { readonly item: NavItem }): ReactNode {
  return (
    <>
      <header className="flex flex-col gap-1">
        <h1 className="text-xl font-semibold">{item.label}</h1>
        <p className="text-sm text-fg-muted">{item.summary}</p>
      </header>
      <StateView
        state={{
          kind: "empty",
          title: "No agents found",
          hint: "Start a Claude Code or pi session, or add an agent root in Settings, and usage will appear here.",
        }}
      />
      {item.to === "/settings" ? <ThemeSwitcher /> : null}
    </>
  );
}
