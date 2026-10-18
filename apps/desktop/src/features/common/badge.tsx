import { AlertTriangle, CheckCircle2, Info, XCircle } from "lucide-react";
import type { ReactNode } from "react";

export type Tone = "neutral" | "success" | "warning" | "danger";

function icon(tone: Tone): ReactNode {
  switch (tone) {
    case "neutral":
      return <Info aria-hidden className="size-3.5" />;
    case "success":
      return <CheckCircle2 aria-hidden className="size-3.5" />;
    case "warning":
      return <AlertTriangle aria-hidden className="size-3.5" />;
    case "danger":
      return <XCircle aria-hidden className="size-3.5" />;
    default:
      return tone satisfies never;
  }
}

function toneClass(tone: Tone): string {
  switch (tone) {
    case "neutral":
      return "text-fg-muted";
    case "success":
      return "text-success";
    case "warning":
      return "text-warning";
    case "danger":
      return "text-danger";
    default:
      return tone satisfies never;
  }
}

/** A short status label. The icon and the text carry the meaning, the colour only reinforces. */
export function Badge({
  tone,
  children,
}: {
  readonly tone: Tone;
  readonly children: ReactNode;
}): ReactNode {
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-md border border-border px-1.5 py-0.5 text-xs font-medium whitespace-nowrap ${toneClass(tone)}`}
    >
      {icon(tone)}
      {children}
    </span>
  );
}
