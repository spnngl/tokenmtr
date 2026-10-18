import { useParams } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { z } from "zod";
import { OverviewScreen } from "../../features/overview/overview-screen";
import { ProjectsScreen } from "../../features/projects/projects-screen";
import { QuotaScreen } from "../../features/quota/quota-screen";
import { SessionDetailScreen } from "../../features/sessions/session-detail-screen";
import { SessionsScreen } from "../../features/sessions/sessions-screen";
import { SettingsScreen } from "../../features/settings/settings-screen";
import { StateView } from "../../shared/state-view";

/** Route components owned by part A. Part B's screens live in their own `routes/<screen>.tsx`. */
export function OverviewRoute(): ReactNode {
  return <OverviewScreen />;
}

export function QuotaRoute(): ReactNode {
  return <QuotaScreen />;
}

export function SessionsRoute(): ReactNode {
  return <SessionsScreen />;
}

export function ProjectsRoute(): ReactNode {
  return <ProjectsScreen />;
}

export function SettingsRoute(): ReactNode {
  return <SettingsScreen />;
}

const sessionParamsSchema = z.object({ agent: z.string().min(1), session: z.string().min(1) });

export function SessionDetailRoute(): ReactNode {
  const params = sessionParamsSchema.safeParse(useParams({ strict: false }));
  if (!params.success) {
    return (
      <StateView
        state={{ kind: "empty", title: "Session not found", hint: "The link is incomplete." }}
      >
        {null}
      </StateView>
    );
  }
  return <SessionDetailScreen agent={params.data.agent} session={params.data.session} />;
}
