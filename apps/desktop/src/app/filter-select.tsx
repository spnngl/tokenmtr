import { Check, ChevronDown } from "lucide-react";
import type { ReactNode } from "react";
import {
  Button,
  Label,
  ListBox,
  ListBoxItem,
  Popover,
  Select,
  SelectValue,
} from "react-aria-components";

interface FilterSelectProps<T extends string> {
  readonly label: string;
  readonly options: readonly { readonly value: T; readonly label: string }[];
  readonly value: T;
  readonly onChange: (value: T) => void;
}

/** A labelled single-choice select with a keyboard-complete popover list. */
export function FilterSelect<T extends string>({
  label,
  options,
  value,
  onChange,
}: FilterSelectProps<T>): ReactNode {
  return (
    <Select
      selectedKey={value}
      onSelectionChange={(key) => {
        const next = options.find((option) => option.value === key);
        if (next !== undefined) {
          onChange(next.value);
        }
      }}
      className="flex min-w-0 flex-col gap-1 sm:w-40"
    >
      <Label className="text-xs font-medium text-fg-muted">{label}</Label>
      <Button className="flex min-h-9 w-full items-center justify-between gap-2 rounded-md border border-border bg-surface px-3 text-sm data-hovered:bg-surface-muted">
        <SelectValue className="min-w-0 truncate" />
        <ChevronDown className="size-4 shrink-0 text-fg-muted" aria-hidden />
      </Button>
      <Popover className="min-w-(--trigger-width) rounded-md border border-border bg-surface-raised p-1 shadow-lg">
        <ListBox className="max-h-72 overflow-auto outline-hidden">
          {options.map((option) => (
            <ListBoxItem
              key={option.value}
              id={option.value}
              textValue={option.label}
              className="flex min-h-9 cursor-pointer items-center justify-between gap-3 rounded-md px-2 text-sm data-focused:bg-surface-muted"
            >
              {({ isSelected }) => (
                <>
                  <span className="truncate">{option.label}</span>
                  {isSelected ? <Check className="size-4 shrink-0" aria-hidden /> : null}
                </>
              )}
            </ListBoxItem>
          ))}
        </ListBox>
      </Popover>
    </Select>
  );
}
