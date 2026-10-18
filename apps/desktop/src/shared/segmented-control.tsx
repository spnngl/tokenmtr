import type { ReactNode } from "react";
import { ToggleButton, ToggleButtonGroup } from "react-aria-components";

interface SegmentedControlProps<T extends string> {
  readonly label: string;
  readonly options: readonly { readonly value: T; readonly label: string }[];
  readonly value: T;
  readonly onChange: (value: T) => void;
}

/** Single-choice segmented control (a radio group of toggle buttons, arrow keys move). */
export function SegmentedControl<T extends string>({
  label,
  options,
  value,
  onChange,
}: SegmentedControlProps<T>): ReactNode {
  return (
    <ToggleButtonGroup
      aria-label={label}
      selectionMode="single"
      disallowEmptySelection
      selectedKeys={[value]}
      onSelectionChange={(keys) => {
        const next = options.find((option) => keys.has(option.value));
        if (next !== undefined) {
          onChange(next.value);
        }
      }}
      className="inline-flex rounded-md border border-border bg-surface-muted p-0.5"
    >
      {options.map((option) => (
        <ToggleButton
          key={option.value}
          id={option.value}
          className="min-h-8 rounded-md px-3 text-sm font-medium text-fg-muted transition-colors data-hovered:text-fg data-selected:bg-surface-raised data-selected:text-fg data-selected:shadow-sm"
        >
          {option.label}
        </ToggleButton>
      ))}
    </ToggleButtonGroup>
  );
}
