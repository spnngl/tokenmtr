import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
} from "react";
import { z } from "zod";

/**
 * Theme setting. `system` removes `data-theme` from `<html>`, so the stylesheet follows
 * `prefers-color-scheme` live; `light` and `dark` pin it. The choice is stored in
 * `localStorage` under `theme`, which the tray and main webviews share.
 */
export const THEMES = ["system", "light", "dark"] as const;
export type Theme = (typeof THEMES)[number];

const STORAGE_KEY = "theme";
const themeSchema = z.enum(THEMES);

function readStoredTheme(): Theme {
  try {
    const parsed = themeSchema.safeParse(localStorage.getItem(STORAGE_KEY));
    return parsed.success ? parsed.data : "system";
  } catch {
    return "system"; // storage blocked: the setting simply does not persist
  }
}

function storeTheme(theme: Theme): void {
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // storage blocked or full: keep the in-memory choice
  }
}

function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.dataset.theme = theme;
  }
}

interface ThemeContextValue {
  readonly theme: Theme;
  readonly setTheme: (theme: Theme) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

/** Owns the theme setting and applies it to `<html>`. */
export function ThemeProvider({ children }: { readonly children: ReactNode }): ReactNode {
  const [theme, setThemeState] = useState<Theme>(readStoredTheme);

  useLayoutEffect(() => {
    applyTheme(theme);
  }, [theme]);

  // Another webview (tray or main) changed the setting.
  useEffect(() => {
    const onStorage = (event: StorageEvent): void => {
      if (event.key === STORAGE_KEY || event.key === null) {
        setThemeState(readStoredTheme());
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next);
    storeTheme(next);
  }, []);

  const value = useMemo(() => ({ theme, setTheme }), [theme, setTheme]);
  return <ThemeContext value={value}>{children}</ThemeContext>;
}

/**
 * The theme setting and its setter.
 * @throws {Error} outside a `ThemeProvider`.
 */
export function useTheme(): ThemeContextValue {
  const value = useContext(ThemeContext);
  if (value === null) {
    throw new Error("useTheme must be used inside ThemeProvider");
  }
  return value;
}
