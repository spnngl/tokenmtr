import type { ReactNode } from "react";
import { useConnectKeychain } from "./actions";
import { Button } from "./button";
import { formatClock, formatRelativeReset } from "./format";
import type { QuotaEta, QuotaMeter, QuotaSpend, QuotaStatus, QuotaWindow } from "./schemas";
import { Amount } from "./ui";
import { UsageMeter } from "./usage-meter";

const WINDOW_LABELS: Readonly<Record<string, string>> = {
  five_hour: "5-hour window",
  seven_day: "7-day window",
  extra_usage: "Extra usage",
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
  keychain_denied:
    'The Keychain did not let the daemon read the Claude credentials. Connect the account and choose "Always Allow" when macOS asks.',
};

/** Asks the daemon to read the Keychain once with the macOS prompt allowed. */
function ConnectKeychain(): ReactNode {
  const connect = useConnectKeychain();
  return (
    <div>
      <Button isPending={connect.isPending} onPress={() => connect.mutate()}>
        Connect Claude account
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

function WindowMeter(props: {
  readonly window: QuotaWindow;
  readonly timeZone: string;
  readonly now: number;
}): ReactNode {
  const { window } = props;
  const reset =
    window.resets_at === null ? null : formatRelativeReset(Date.parse(window.resets_at), props.now);
  const eta = etaText(window.eta, props.timeZone);
  const timing = [reset, eta].filter((part) => part !== null).join(" · ");
  // The extra-usage window has spend and never a reset: its detail is the spend alone.
  return (
    <UsageMeter
      label={windowLabel(window.window)}
      usedFraction={window.used_percent / 100}
      detail={window.spend === null ? timing : <SpendDetail spend={window.spend} />}
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
            <>
              <p role="status" className="text-sm text-fg-muted">
                {PROBLEMS[meter.status]}
              </p>
              {meter.status === "keychain_denied" ? <ConnectKeychain /> : null}
            </>
          )}
        </div>
      ))}
    </div>
  );
}
