import { Link, Outlet } from "@tanstack/react-router";
import { Menu, X } from "lucide-react";
import { type ReactNode, useState } from "react";
import { Dialog, DialogTrigger, Modal, ModalOverlay } from "react-aria-components";
import { APP_NAME } from "../shared/brand";
import { BrandMark } from "../shared/brand-mark";
import { Button } from "../shared/button";
import { NAV_ITEMS } from "../shared/nav";
import { AppBanners, AppGate } from "./app-gate";
import { FilterBar } from "./filter-bar";
import { CommandPalette } from "./palette";

function NavList({ onNavigate }: { readonly onNavigate?: () => void }): ReactNode {
  return (
    <nav aria-label="Primary">
      <ul className="flex flex-col gap-0.5">
        {NAV_ITEMS.map((item) => (
          <li key={item.to}>
            <Link
              to={item.to}
              // Keep the filters when moving between screens.
              search={(previous: Record<string, unknown>) => previous}
              activeOptions={{ exact: item.to === "/" }}
              {...(onNavigate === undefined ? {} : { onClick: onNavigate })}
              className="flex min-h-10 items-center gap-3 rounded-md px-3 text-sm font-medium text-fg-muted hover:bg-surface-muted hover:text-fg data-[status=active]:bg-surface-muted data-[status=active]:text-fg"
            >
              <item.icon className="size-4 shrink-0" aria-hidden />
              {item.label}
            </Link>
          </li>
        ))}
      </ul>
    </nav>
  );
}

function Brand(): ReactNode {
  return (
    <span className="flex items-center gap-2 text-base font-semibold">
      <BrandMark className="size-6 shrink-0" />
      {APP_NAME}
    </span>
  );
}

/** Below `md` the sidebar becomes a drawer opened from the top bar. */
function NavDrawer(): ReactNode {
  const [isOpen, setOpen] = useState(false);
  return (
    <DialogTrigger isOpen={isOpen} onOpenChange={setOpen}>
      <Button variant="quiet" aria-label="Open navigation">
        <Menu className="size-5" aria-hidden />
      </Button>
      <ModalOverlay isDismissable className="fixed inset-0 z-20 bg-scrim">
        <Modal className="fixed inset-y-0 left-0 w-72 max-w-[85vw] border-r border-border bg-surface-raised p-3 outline-hidden">
          <Dialog aria-label="Navigation" className="flex h-full flex-col gap-3 outline-hidden">
            <div className="flex items-center justify-between">
              <Brand />
              <Button variant="quiet" aria-label="Close navigation" onPress={() => setOpen(false)}>
                <X className="size-5" aria-hidden />
              </Button>
            </div>
            <NavList onNavigate={() => setOpen(false)} />
          </Dialog>
        </Modal>
      </ModalOverlay>
    </DialogTrigger>
  );
}

/** Target of the skip link: the routed screen. */
const MAIN_ID = "main-content";

/**
 * Full-app shell: sidebar (drawer when narrow), top bar with the command palette, then the gate:
 * filter bar and routed screen when the app can show data, otherwise the state that explains why not.
 */
export function Layout(): ReactNode {
  return (
    <div className="flex min-h-dvh flex-col md:flex-row">
      {/* A hash router owns the URL fragment, so the skip link moves focus instead of linking. */}
      <button
        type="button"
        onClick={() => document.getElementById(MAIN_ID)?.focus()}
        className="sr-only z-50 rounded-md bg-surface-raised px-3 py-2 text-sm font-medium text-fg shadow-sm focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
      >
        Skip to content
      </button>
      <aside className="sticky top-0 hidden h-dvh w-56 shrink-0 flex-col gap-4 border-r border-border bg-surface-raised p-3 md:flex">
        <div className="px-3 pt-1">
          <Brand />
        </div>
        <NavList />
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center gap-2 border-b border-border bg-surface-raised p-2">
          <div className="flex items-center gap-2 md:hidden">
            <NavDrawer />
            <Brand />
          </div>
          <div className="ml-auto">
            <CommandPalette />
          </div>
        </header>
        <AppGate>
          <FilterBar />
          <main
            id={MAIN_ID}
            tabIndex={-1}
            className="flex min-w-0 flex-1 flex-col gap-4 p-4 outline-none md:p-6"
          >
            <AppBanners>
              <Outlet />
            </AppBanners>
          </main>
        </AppGate>
      </div>
    </div>
  );
}
