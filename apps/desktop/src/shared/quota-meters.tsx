import type { CSSProperties, ReactNode } from "react";
import { Label, Meter } from "react-aria-components";
import { useReloadClaudeCredentials } from "./actions";
import { Button } from "./button";
import { formatClock, formatPercent, formatRelativeReset } from "./format";
import type { QuotaEta, QuotaMeter, QuotaSpend, QuotaStatus, QuotaWindow } from "./schemas";
import { Amount } from "./ui";
import { LevelIcon, type MeterLevel, meterLevel } from "./usage-meter";

const PROBLEMS: Readonly<Record<Exclude<QuotaStatus, "ok">, string>> = {
  signed_out: "Signed out. Sign in to the agent to see its quota.",
  token_expired:
    "Token expired. Use the agent once so it refreshes the token: the next poll picks it up, or press Refresh.",
  failed: "Could not fetch quota. Retrying on the next scan.",
  keychain_denied:
    'The Keychain did not let the daemon read the Claude credentials. Connect the account and choose "Always Allow" when macOS asks.',
};

/**
 * Asks the daemon to re-read the credentials and poll again: for a token the agent refreshed
 * since the last poll, or a Keychain that refused the daemon (macOS then asks the user).
 */
function ReloadCredentials(props: { readonly status: Exclude<QuotaStatus, "ok"> }): ReactNode {
  const reload = useReloadClaudeCredentials();
  return (
    <div>
      <Button isPending={reload.isPending} onPress={() => reload.mutate()}>
        {props.status === "keychain_denied" ? "Connect Claude account" : "Refresh"}
      </Button>
    </div>
  );
}

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

/** Claude extra usage: "$12.35 of $50.00 this month" (approximate after an old FX rate). */
function SpendDetail(props: { readonly spend: QuotaSpend }): ReactNode {
  const { spend } = props;
  return (
    <>
      <Amount
        micros={spend.spent_micros}
        currency={spend.currency}
        approximate={spend.approximate}
      />
      {" of "}
      <Amount
        micros={spend.limit_micros}
        currency={spend.currency}
        approximate={spend.approximate}
      />
      {" this month"}
    </>
  );
}

const ARC: Record<MeterLevel, string> = {
  ok: "text-accent",
  warning: "text-warning",
  danger: "text-danger",
};

/** A circle of circumference 100, so a dash length is a percentage. */
const RADIUS = 100 / (2 * Math.PI);

/**
 * Profiles side by side from 320 px each; donuts side by side inside a profile, narrow enough
 * that a profile's four windows (three plus extra usage) share one row in the 360 px recap.
 */
const GROUPS: CSSProperties = {
  gridTemplateColumns: "repeat(auto-fill, minmax(min(100%, 20rem), 1fr))",
};
const DONUTS: CSSProperties = { gridTemplateColumns: "repeat(auto-fill, minmax(4.5rem, 1fr))" };

/**
 * One window as a donut: used percent in the centre, label and reset (or spend) below. A meter
 * named by the label whose value text adds the reset; past 100% the ring is full and the centre
 * keeps the true percentage.
 */
function WindowDonut(props: {
  readonly window: QuotaWindow;
  readonly timeZone: string;
  readonly now: number;
}): ReactNode {
  const { window } = props;
  const used = window.used_percent / 100;
  const level = meterLevel(used);
  const reset =
    window.resets_at === null ? null : formatRelativeReset(Date.parse(window.resets_at), props.now);
  const eta = etaText(window.eta, props.timeZone);
  const valueLabel = [`${formatPercent(used)} used`, reset]
    .filter((part) => part !== null)
    .join(", ");
  const filled = Math.min(Math.max(window.used_percent, 0), 100);
  return (
    <Meter
      value={filled}
      valueLabel={valueLabel}
      className="flex flex-col items-center gap-1 text-center"
    >
      <div className="relative size-16">
        <svg viewBox="0 0 36 36" className="size-full -rotate-90" aria-hidden>
          <circle
            cx="18"
            cy="18"
            r={RADIUS}
            fill="none"
            strokeWidth="3.5"
            className="stroke-current text-surface-muted"
          />
          {filled === 0 ? null : (
            <circle
              cx="18"
              cy="18"
              r={RADIUS}
              fill="none"
              strokeWidth="3.5"
              strokeDasharray={`${filled} ${100 - filled}`}
              className={`stroke-current ${ARC[level]}`}
            />
          )}
        </svg>
        <span className="absolute inset-0 flex flex-col items-center justify-center text-sm font-semibold tabular-nums">
          <LevelIcon level={level} />
          {formatPercent(used)}
        </span>
      </div>
      <Label className="text-sm font-medium">{window.label}</Label>
      {/* The extra-usage window has spend and never a reset: its detail is the spend alone. */}
      <p className="text-xs text-fg-muted">
        {window.spend === null ? reset : <SpendDetail spend={window.spend} />}
      </p>
      {window.spend === null && eta !== null ? (
        <p className="text-xs text-fg-muted">{eta}</p>
      ) : null}
    </Meter>
  );
}

/**
 * Quota donuts of every profile that reports quota, one group per agent profile. Renders nothing
 * for an empty list, so an agent without the capability shows no block and no error.
 */
export function QuotaMeters(props: {
  readonly meters: readonly QuotaMeter[];
  readonly timeZone: string;
  readonly now: number;
}): ReactNode {
  if (props.meters.length === 0) {
    return null;
  }
  return (
    <div className="grid gap-6" style={GROUPS}>
      {props.meters.map((meter) => {
        const owner = `${meter.agent} / ${meter.profile}`;
        return (
          <section key={owner} aria-label={owner} className="flex flex-col gap-3">
            <h3 className="text-xs font-medium text-fg-muted">
              {owner}
              {meter.account === null ? "" : ` (${meter.account})`}
            </h3>
            {meter.status === "ok" ? (
              <div className="grid gap-2" style={DONUTS}>
                {meter.windows.map((window) => (
                  <WindowDonut
                    key={window.window}
                    window={window}
                    timeZone={props.timeZone}
                    now={props.now}
                  />
                ))}
              </div>
            ) : (
              <>
                <p role="status" className="text-sm text-fg-muted">
                  {PROBLEMS[meter.status]}
                </p>
                <ReloadCredentials status={meter.status} />
              </>
            )}
          </section>
        );
      })}
    </div>
  );
}
