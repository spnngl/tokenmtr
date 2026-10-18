import type { ReactNode } from "react";
import { Button as AriaButton, type ButtonProps as AriaButtonProps } from "react-aria-components";

type Variant = "primary" | "secondary" | "quiet";

const variantClass: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg data-hovered:bg-accent/90 data-pressed:bg-accent/80",
  secondary:
    "border border-border bg-surface-raised text-fg data-hovered:bg-surface-muted data-pressed:bg-border",
  quiet: "text-fg data-hovered:bg-surface-muted data-pressed:bg-border",
};

interface ButtonProps extends Omit<AriaButtonProps, "className" | "children"> {
  readonly variant?: Variant;
  readonly children: ReactNode;
}

/** The app's button: token-driven variants over React Aria; focus ring comes from the base layer. */
export function Button({ variant = "secondary", children, ...props }: ButtonProps): ReactNode {
  return (
    <AriaButton
      {...props}
      className={`inline-flex min-h-9 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-colors data-disabled:opacity-50 ${variantClass[variant]}`}
    >
      {children}
    </AriaButton>
  );
}
