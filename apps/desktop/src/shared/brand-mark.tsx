import type { ReactNode } from "react";

/**
 * The TokenMtr mark (assets/brand/tokenmtr-logo.svg). Decorative: the product name always sits
 * next to it as text. Brand colors are fixed on purpose; they read on both themes.
 */
export function BrandMark({ className }: { readonly className?: string }): ReactNode {
  return (
    <svg viewBox="8 8 84 84" className={className} aria-hidden focusable="false">
      <g fill="none" strokeWidth="6" strokeLinecap="butt">
        <path d="M 83.8 37.7 A 36 36 0 1 1 62.3 16.2" stroke="#e05273" />
        <path d="M 71.3 44.3 A 22 22 0 1 1 55.7 28.7" stroke="#6b94b7" />
        <circle cx="50" cy="50" r="9" stroke="#6b94b7" />
        <line x1="56.4" y1="43.6" x2="83.2" y2="16.8" stroke="#6b94b7" />
      </g>
    </svg>
  );
}
