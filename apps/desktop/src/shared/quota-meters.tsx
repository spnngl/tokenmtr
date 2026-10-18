import type { ReactNode } from "react";
import { formatClock, formatRelativeReset } from "./format";
import type { QuotaEta, QuotaMeter, QuotaStatus, QuotaWindow } from "./schemas";
import { UsageMeter } from "./usage-meter";

const WINDOW_LABELS: Readonly<Record<string, string>> = {
  five_hour: "5-hour window",
  seven_day: "7-day window",
};

/** `five_hour` -> "5-hour window"; unknown windows are title-cased rather than hidden. */
export function windowLabel(window: string): string {
  const known = WINDOW_LABELS[window];
  if (known !== undefined) {
    return known;
  }
  const words = window.replaceAll("_", " ");
  return `${words.charAt(0).toUpperCase()}${words.slice(1)}`;
}

const PROBLEMS: Readonly<Record<Exclude<QuotaStatus, "ok">, string>> = {
  signed_out: "Signed out. Sign in to the agent to see its quota.",
  token_expired: "Token expired. Open the agent once to refresh it.",
  failed: "Could not fetch quota. Retrying on the next scan.",
};

export function etaText(eta: QuotaEta, timeZone: string): string | null {
  switch (eta.kind) {
    case "will_hit":
      return `100% at ${formatClock(Date.parse(eta.at), timeZone)} at this pace`;
    case "wont_hit":
      return "Will not reach 100% before reset";
    case "not_enough_data":
      return null;
    default:
      eta satisfies never;
      throw new Error("unreachable eta");
  }
}

function WindowMeter(props: {
  readonly window: QuotaWindow;
  readonly timeZone: string;
  readonly now: number;
}): ReactNode {
  const { window } = props;
  const reset =
    window.resets_at === null ? null : formatRelativeReset(Date.parse(window.resets_at), props.now);
  const eta = etaText(window.eta, props.timeZone);
  return (
    <UsageMeter
      label={windowLabel(window.window)}
      usedFraction={window.used_percent / 100}
      detail={[reset, eta].filter((part) => part !== null).join(" · ")}
    />
  );
}

/**
 * Quota meters of every profile that reports quota. Renders nothing for an empty list, so an
 * agent without the capability shows no block and no error.
 */
export function QuotaMeters(props: {
  readonly meters: readonly QuotaMeter[];
  readonly timeZone: string;
  readonly now: number;
}): ReactNode {
  if (props.meters.length === 0) {
    return null;
  }
  const showOwner = props.meters.length > 1;
  return (
    <div className="flex flex-col gap-4">
      {props.meters.map((meter) => (
        <div key={`${meter.agent}:${meter.profile}`} className="flex flex-col gap-3">
          {showOwner ? (
            <p className="text-xs font-medium text-fg-muted">
              {meter.agent} / {meter.profile}
              {meter.account === null ? "" : ` (${meter.account})`}
            </p>
          ) : null}
          {meter.status === "ok" ? (
            meter.windows.map((window) => (
              <WindowMeter
                key={window.window}
                window={window}
                timeZone={props.timeZone}
                now={props.now}
              />
            ))
          ) : (
            <p role="status" className="text-sm text-fg-muted">
              {PROBLEMS[meter.status]}
            </p>
          )}
        </div>
      ))}
    </div>
  );
}
