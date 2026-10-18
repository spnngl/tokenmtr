import { useLocation, useNavigate } from "@tanstack/react-router";
import { useCallback, useMemo } from "react";
import { useAppConfig } from "../shared/app-context";
import {
  defaultFilters,
  type Filters,
  type ParsedFilters,
  parseFilters,
  serializeFilters,
} from "../shared/filters";

interface UseFilters extends ParsedFilters {
  /** Writes `next` to the URL, replacing the history entry so filtering does not fill Back. */
  readonly setFilters: (next: Filters) => void;
}

/** The filters in the current URL, and a setter that writes them back. */
export function useFilters(): UseFilters {
  const { defaultCurrency } = useAppConfig();
  const searchStr = useLocation({ select: (location) => location.searchStr });
  const navigate = useNavigate();
  const defaults = useMemo(() => defaultFilters(defaultCurrency), [defaultCurrency]);
  const parsed = useMemo(
    () => parseFilters(new URLSearchParams(searchStr), defaults),
    [searchStr, defaults],
  );
  const setFilters = useCallback(
    (next: Filters) => {
      void navigate({
        to: ".",
        search: Object.fromEntries(serializeFilters(next, defaults)),
        replace: true,
      });
    },
    [navigate, defaults],
  );
  return { ...parsed, setFilters };
}
