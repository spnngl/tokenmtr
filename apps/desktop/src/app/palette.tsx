import { useNavigate } from "@tanstack/react-router";
import { Search } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import {
  Autocomplete,
  Dialog,
  Header,
  Input,
  Menu,
  MenuItem,
  MenuSection,
  Modal,
  ModalOverlay,
  SearchField,
} from "react-aria-components";
import { useBackupNow, useExportNow, useScanNow } from "../shared/actions";
import { Button } from "../shared/button";
import { EMPTY_FILTER_OPTIONS, useFilterOptions, useOs } from "../shared/queries";
import { useTheme } from "../shared/theme";
import {
  buildPaletteItems,
  filterItems,
  type PaletteCommand,
  type PaletteGroup,
  type PaletteItem,
  runCommand,
} from "./palette-model";
import { useFilters } from "./use-filters";

const GROUPS: readonly PaletteGroup[] = ["Go to", "Filter", "Action"];

interface PaletteProps {
  readonly items: readonly PaletteItem[];
  readonly isOpen: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onChoose: (command: PaletteCommand) => void;
}

/** The Cmd-K dialog: a search field filtering one grouped list; Enter or click runs the item. */
function Palette({ items, isOpen, onOpenChange, onChoose }: PaletteProps): ReactNode {
  const [query, setQuery] = useState("");
  const shown = useMemo(() => filterItems(items, query), [items, query]);
  const choose = (id: PaletteItem["id"]): void => {
    const item = items.find((candidate) => candidate.id === id);
    if (item !== undefined) {
      onOpenChange(false);
      setQuery("");
      onChoose(item.command);
    }
  };

  return (
    <ModalOverlay
      isOpen={isOpen}
      onOpenChange={(open) => {
        if (!open) {
          setQuery("");
        }
        onOpenChange(open);
      }}
      isDismissable
      className="fixed inset-0 z-30 flex items-start justify-center bg-scrim p-4 pt-[12vh]"
    >
      <Modal className="w-full max-w-lg overflow-hidden rounded-md border border-border bg-surface-raised shadow-lg outline-hidden">
        <Dialog aria-label="Command palette" className="flex flex-col outline-hidden">
          <Autocomplete inputValue={query} onInputChange={setQuery}>
            <SearchField
              aria-label="Search commands"
              autoFocus
              className="flex items-center gap-2 border-b border-border px-3"
            >
              <Search className="size-4 shrink-0 text-fg-muted" aria-hidden />
              <Input
                placeholder="Go to, filter or run…"
                className="min-h-11 min-w-0 flex-1 bg-transparent text-sm outline-hidden placeholder:text-fg-muted"
              />
            </SearchField>
            <Menu
              aria-label="Commands"
              onAction={(key) => choose(String(key))}
              renderEmptyState={() => (
                <p className="p-4 text-sm text-fg-muted">No matching command.</p>
              )}
              className="max-h-80 overflow-y-auto p-1 outline-hidden"
            >
              {GROUPS.map((group) => {
                const inGroup = shown.filter((item) => item.group === group);
                return inGroup.length === 0 ? null : (
                  <MenuSection key={group}>
                    <Header className="px-2 pt-2 pb-1 text-xs font-medium text-fg-muted">
                      {group}
                    </Header>
                    {inGroup.map((item) => (
                      <MenuItem
                        key={item.id}
                        id={item.id}
                        textValue={item.label}
                        className="flex min-h-9 cursor-pointer items-center rounded-sm px-2 text-sm outline-hidden data-focused:bg-surface-muted data-hovered:bg-surface-muted"
                      >
                        {item.label}
                      </MenuItem>
                    ))}
                  </MenuSection>
                );
              })}
            </Menu>
          </Autocomplete>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

/** Opens on Cmd/Ctrl+K from anywhere in the app; `toggle` must be stable (one listener). */
function useOpenShortcut(toggle: () => void): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [toggle]);
}

/** The palette wired to the router, filters, theme and data-source actions, plus its trigger. */
export function CommandPalette(): ReactNode {
  const [isOpen, setOpen] = useState(false);
  const { filters, setFilters } = useFilters();
  const { data } = useFilterOptions();
  const navigate = useNavigate();
  const { setTheme } = useTheme();
  const os = useOs();
  // Outcomes land in the app's toast region: the palette is closed by the time they arrive.
  const scan = useScanNow();
  const exportNow = useExportNow();
  const backup = useBackupNow();
  const items = useMemo(
    () => buildPaletteItems(data ?? EMPTY_FILTER_OPTIONS, filters),
    [data, filters],
  );
  useOpenShortcut(useCallback(() => setOpen((open) => !open), []));

  return (
    <>
      <Button variant="secondary" onPress={() => setOpen(true)}>
        <Search className="size-4" aria-hidden />
        <span>Search</span>
        <kbd className="hidden rounded-sm border border-border px-1 text-xs text-fg-muted md:inline">
          {os === "macos" ? "⌘K" : "Ctrl+K"}
        </kbd>
      </Button>
      <Palette
        items={items}
        isOpen={isOpen}
        onOpenChange={setOpen}
        onChoose={(command) =>
          runCommand(command, {
            filters,
            // Keep the filters when moving between screens.
            navigate: (to) =>
              void navigate({ to, search: (previous: Record<string, unknown>) => previous }),
            setFilters,
            setTheme,
            scanNow: () => scan.mutate(),
            exportNow: () => exportNow.mutate(),
            backupNow: () => backup.mutate(),
          })
        }
      />
    </>
  );
}
