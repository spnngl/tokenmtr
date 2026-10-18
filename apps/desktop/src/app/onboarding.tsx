import { Check } from "lucide-react";
import type { ReactNode } from "react";
import { useDataAction } from "../shared/actions";
import { Button } from "../shared/button";
import type { AgentInfo, AppState } from "../shared/schemas";
import { Card } from "../shared/ui";

type OnboardingState = Extract<AppState, { kind: "onboarding" }>;

const NOTIFICATION_TEXT = {
  granted: "Notifications are on.",
  denied: "Notifications are blocked. Allow them in system settings to get quota alerts.",
  undetermined: "Get an alert before a quota window fills up or a budget is reached.",
} as const;

function capabilityList(agent: AgentInfo): string {
  const { capabilities } = agent;
  const names = [
    capabilities.quota ? "quota" : null,
    capabilities.reported_cost ? "reported cost" : null,
    capabilities.subscription ? "subscription" : null,
  ].filter((name) => name !== null);
  return names.length === 0 ? "usage only" : names.join(", ");
}

function ActionError({ error }: { readonly error: Error | null }): ReactNode {
  return error === null ? null : (
    <p role="alert" className="text-sm text-danger">
      {error.message}
    </p>
  );
}

function Done({ children }: { readonly children: ReactNode }): ReactNode {
  return (
    <p className="flex items-center gap-2 text-sm text-fg-muted">
      <Check className="size-4 text-success" aria-hidden />
      {children}
    </p>
  );
}

/** First run (PLAN §7): what was found, then the three one-time choices, then go. */
export function Onboarding({ state }: { readonly state: OnboardingState }): ReactNode {
  const install = useDataAction((ds) => ds.installDaemon());
  const notify = useDataAction((ds) => ds.requestNotifications());
  const keep = useDataAction((ds, v: { agent: string; profile: string }) =>
    ds.keepTenYears(v.agent, v.profile),
  );
  const finish = useDataAction((ds) => ds.completeOnboarding());
  const found = state.agents.filter((agent) => agent.profiles.length > 0);

  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-4 p-4 md:p-6">
      <div className="flex flex-col gap-1">
        <h1 className="text-xl font-semibold">Welcome to TokenMtr</h1>
        <p className="text-sm text-fg-muted">
          Local usage, cost and quota for your coding agents. Nothing leaves this machine.
        </p>
      </div>

      <Card title="Detected agents">
        {found.length === 0 ? (
          <p className="text-sm text-fg-muted">
            No agent found yet. Run a session with a supported agent, then scan again.
          </p>
        ) : (
          <ul className="flex flex-col gap-3">
            {found.map((agent) => (
              <li key={agent.agent} className="flex flex-col gap-1">
                <p className="text-sm font-medium">
                  {agent.label}{" "}
                  <span className="font-normal text-fg-muted">({capabilityList(agent)})</span>
                </p>
                <ul className="flex flex-col gap-1">
                  {agent.profiles.map((profile) => (
                    <li key={profile.profile} className="text-sm text-fg-muted">
                      <span className="font-medium text-fg">{profile.profile}</span> ·{" "}
                      {profile.root}
                      {profile.account === null ? "" : ` · ${profile.account}`}
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        )}
      </Card>

      {found.flatMap((agent) =>
        agent.profiles
          .filter((profile) => profile.retention_days !== null)
          .map((profile) => (
            <Card
              key={`${agent.agent}:${profile.profile}`}
              title={`Keep ${agent.label} (${profile.profile}) history`}
            >
              {profile.keeps_ten_years ? (
                <Done>{profile.profile}: transcripts are kept for 10 years.</Done>
              ) : (
                <>
                  <p className="text-sm text-fg-muted">
                    {agent.label} deletes {profile.profile} transcripts after{" "}
                    {profile.retention_days} days, and usage older than that is lost. Raising the
                    limit to 10 years edits the agent's own settings file.
                  </p>
                  <div>
                    <Button
                      variant="secondary"
                      isPending={keep.isPending}
                      onPress={() => keep.mutate({ agent: agent.agent, profile: profile.profile })}
                    >
                      Keep for 10 years
                    </Button>
                  </div>
                  <ActionError error={keep.error} />
                </>
              )}
            </Card>
          )),
      )}

      <Card title="Background daemon">
        {state.daemon_installed ? (
          <Done>The daemon is installed and scans in the background.</Done>
        ) : (
          <>
            <p className="text-sm text-fg-muted">
              The daemon keeps usage up to date while the app is closed.
            </p>
            <div>
              <Button
                variant="secondary"
                isPending={install.isPending}
                onPress={() => install.mutate()}
              >
                Install daemon
              </Button>
            </div>
            <ActionError error={install.error} />
          </>
        )}
      </Card>

      <Card title="Notifications">
        <p className="text-sm text-fg-muted">{NOTIFICATION_TEXT[state.notifications]}</p>
        {state.notifications === "undetermined" ? (
          <div>
            <Button
              variant="secondary"
              isPending={notify.isPending}
              onPress={() => notify.mutate()}
            >
              Allow notifications
            </Button>
          </div>
        ) : null}
        <ActionError error={notify.error} />
      </Card>

      <div className="flex flex-col items-start gap-2">
        <Button variant="primary" isPending={finish.isPending} onPress={() => finish.mutate()}>
          Open dashboard
        </Button>
        <ActionError error={finish.error} />
      </div>
    </div>
  );
}
