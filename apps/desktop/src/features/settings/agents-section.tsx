import type { ReactNode } from "react";
import { useDataAction } from "../../shared/actions";
import { Button } from "../../shared/button";
import type { AgentInfo, Capabilities } from "../../shared/schemas";
import { Card } from "../../shared/ui";

const CAPABILITY_LABELS: readonly { key: keyof Capabilities; label: string }[] = [
  { key: "quota", label: "Quota" },
  { key: "reported_cost", label: "Reported cost" },
  { key: "subscription", label: "Subscription" },
];

function capabilityText(capabilities: Capabilities): string {
  const names = CAPABILITY_LABELS.filter(({ key }) => capabilities[key]).map(({ label }) => label);
  return names.length === 0 ? "Usage only" : names.join(", ");
}

function ProfileRow({
  agent,
  profile,
}: {
  readonly agent: AgentInfo;
  readonly profile: AgentInfo["profiles"][number];
}): ReactNode {
  const keep = useDataAction((ds) => ds.keepTenYears(agent.agent, profile.profile));
  return (
    <li className="flex flex-col gap-1 border-t border-border pt-2 first:border-t-0 first:pt-0">
      <p className="text-sm">
        <span className="font-medium">{profile.profile}</span>
        {profile.account === null ? null : (
          <span className="text-fg-muted"> · {profile.account}</span>
        )}
      </p>
      <p className="text-xs text-fg-muted">{profile.root}</p>
      {profile.retention_days === null ? null : (
        <div className="flex flex-wrap items-center gap-2 text-sm">
          {profile.keeps_ten_years ? (
            <span>Transcripts are kept for 10 years.</span>
          ) : (
            <>
              <span>
                Transcripts are deleted after {profile.retention_days} days; older usage is lost.
              </span>
              <Button isPending={keep.isPending} onPress={() => keep.mutate()}>
                Keep for 10 years
              </Button>
            </>
          )}
          {keep.error === null ? null : (
            <span role="alert" className="text-danger">
              {keep.error.message}
            </span>
          )}
        </div>
      )}
    </li>
  );
}

/** Followed agents, their capabilities and profiles, and per-profile retention (D53). */
export function AgentsSection({ agents }: { readonly agents: readonly AgentInfo[] }): ReactNode {
  return (
    <Card title="Agents">
      {agents.length === 0 ? (
        <p className="text-sm text-fg-muted">No agent has been detected.</p>
      ) : (
        <ul className="flex flex-col gap-4">
          {agents.map((agent) => (
            <li key={agent.agent} className="flex flex-col gap-2">
              <p className="text-sm font-medium">
                {agent.label}{" "}
                <span className="font-normal text-fg-muted">
                  · {agent.followed ? "Followed" : "Not followed"} ·{" "}
                  {capabilityText(agent.capabilities)}
                </span>
              </p>
              <ul className="flex flex-col gap-2">
                {agent.profiles.map((profile) => (
                  <ProfileRow key={profile.profile} agent={agent} profile={profile} />
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
