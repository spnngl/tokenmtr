import { useSyncExternalStore } from "react";

const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

function reducedMotionQuery(): MediaQueryList | null {
  return typeof window.matchMedia === "function" ? window.matchMedia(REDUCED_MOTION) : null;
}

function subscribe(onChange: () => void): () => void {
  const query = reducedMotionQuery();
  query?.addEventListener("change", onChange);
  document.addEventListener("visibilitychange", onChange);
  return () => {
    query?.removeEventListener("change", onChange);
    document.removeEventListener("visibilitychange", onChange);
  };
}

function getSnapshot(): boolean {
  const reduced = reducedMotionQuery()?.matches ?? false;
  return !reduced && document.visibilityState === "visible";
}

/**
 * Whether charts may animate: false while the user asks for reduced motion or the window
 * is hidden. Feed it to every series' `isAnimationActive`.
 */
export function useChartAnimation(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, () => false);
}
