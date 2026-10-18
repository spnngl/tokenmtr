import { Monitor, Moon, Sun } from "lucide-react";
import type { ReactNode } from "react";
import { Label, Radio, RadioGroup } from "react-aria-components";
import { THEMES, type Theme, useTheme } from "./theme";

const OPTIONS: Record<Theme, { readonly label: string; readonly icon: ReactNode }> = {
  system: { label: "System", icon: <Monitor className="size-4" aria-hidden /> },
  light: { label: "Light", icon: <Sun className="size-4" aria-hidden /> },
  dark: { label: "Dark", icon: <Moon className="size-4" aria-hidden /> },
};

/** System / Light / Dark as one single-choice group; System is the only way back to the OS. */
export function ThemeSwitcher(): ReactNode {
  const { theme, setTheme } = useTheme();
  return (
    <RadioGroup
      value={theme}
      orientation="horizontal"
      onChange={(value) => {
        const next = THEMES.find((candidate) => candidate === value);
        if (next !== undefined) {
          setTheme(next);
        }
      }}
      className="flex flex-col gap-1"
    >
      <Label className="text-sm font-medium">Theme</Label>
      <div className="inline-flex w-fit rounded-md border border-border bg-surface-muted p-0.5">
        {THEMES.map((value) => (
          <Radio
            key={value}
            value={value}
            className="flex min-h-8 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm font-medium text-fg-muted transition-colors data-hovered:text-fg data-selected:bg-surface-raised data-selected:text-fg data-selected:shadow-sm"
          >
            {OPTIONS[value].icon}
            {OPTIONS[value].label}
          </Radio>
        ))}
      </div>
    </RadioGroup>
  );
}
