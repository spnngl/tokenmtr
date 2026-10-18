import { type ReactNode, useEffect, useRef, useState } from "react";

/** Width used before the first measurement, and where `ResizeObserver` is missing (jsdom). */
const FALLBACK_WIDTH = 640;

interface ResponsiveChartProps {
  readonly height: number;
  /** Receives the measured pixel size; Recharts needs numbers, percentages are measured once. */
  readonly children: (size: { readonly width: number; readonly height: number }) => ReactNode;
}

/** Gives a chart the width of its container and keeps it in sync on resize. */
export function ResponsiveChart({ height, children }: ResponsiveChartProps): ReactNode {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(FALLBACK_WIDTH);
  useEffect(() => {
    const element = ref.current;
    if (element === null || typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const measured = entries[0]?.contentRect.width ?? 0;
      if (measured > 0) {
        setWidth(Math.floor(measured));
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={ref} className="w-full min-w-0">
      {children({ width, height })}
    </div>
  );
}
