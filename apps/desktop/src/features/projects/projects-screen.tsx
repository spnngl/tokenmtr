import { useNavigate } from "@tanstack/react-router";
import { ArrowLeft } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { useDataAction } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { FormDialog, FormTextField } from "../../shared/form-dialog";
import {
  type Currency,
  formatCount,
  formatDateTime,
  formatPercent,
  formatTokens,
} from "../../shared/format";
import {
  useBranches,
  useProjectRuleFor,
  useProjects,
  useUnresolvedCwds,
} from "../../shared/queries";
import { RankedBars } from "../../shared/ranked-bars";
import { CardRemote, NO_USAGE_COPY, RemoteView } from "../../shared/remote-view";
import type { ProjectRow, UnresolvedCwd } from "../../shared/schemas";
import { Amount, Card, type Column, DataTable, PageHeader } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";

const NO_CLIENT = "No client";

/** Projects grouped by client; the groups keep the order of their biggest project. */
export function groupByClient(rows: readonly ProjectRow[]): [string, ProjectRow[]][] {
  const groups = new Map<string, ProjectRow[]>();
  for (const row of rows) {
    const key = row.client ?? NO_CLIENT;
    groups.set(key, [...(groups.get(key) ?? []), row]);
  }
  return [...groups.entries()];
}

function ProjectBars(props: {
  readonly rows: readonly ProjectRow[];
  readonly currency: Currency;
  readonly label: string;
  readonly onOpen: (project: string) => void;
}): ReactNode {
  return (
    <RankedBars
      label={props.label}
      items={props.rows.map((row) => {
        const { project } = row;
        return {
          key: project ?? "(none)",
          label: row.name ?? project ?? "No project",
          value: (
            <Amount
              micros={row.api_value_micros}
              currency={props.currency}
              approximate={row.approximate}
            />
          ),
          percent: row.share_percent,
          detail: `${formatPercent(row.share_percent / 100)} of cost · ${formatTokens(row.total_tokens)} tokens`,
          ...(project === null ? {} : { onPress: () => props.onOpen(project) }),
        };
      })}
    />
  );
}

/** One project's branches (`/projects/$project`); the filter bar's filters still apply. */
export function ProjectDetailScreen({ project }: { readonly project: string }): ReactNode {
  const { filters } = useFilters();
  const navigate = useNavigate();
  const remote = toRemote(useBranches({ ...filters, project }));
  return (
    <>
      <div>
        <Button
          variant="quiet"
          onPress={() =>
            void navigate({
              to: "/projects",
              search: (previous: Record<string, unknown>) => previous,
            })
          }
        >
          <ArrowLeft className="size-4" aria-hidden />
          All projects
        </Button>
      </div>
      <PageHeader title={project} summary="Cost per git branch." />
      <RemoteView
        remote={remote}
        emptyWhen={({ rows }) => (rows.length === 0 ? NO_USAGE_COPY : null)}
      >
        {({ rows, currency }) => {
          const max = Math.max(1, ...rows.map((row) => row.api_value_micros));
          return (
            <Card title="Branches">
              <RankedBars
                label="Branches by API value"
                items={rows.map((row) => ({
                  key: row.branch ?? "(none)",
                  label: row.branch ?? "No branch",
                  value: (
                    <Amount
                      micros={row.api_value_micros}
                      currency={currency}
                      approximate={row.approximate}
                    />
                  ),
                  percent: (row.api_value_micros / max) * 100,
                  detail: `${formatTokens(row.total_tokens)} tokens`,
                }))}
              />
            </Card>
          );
        }}
      </RemoteView>
    </>
  );
}

/**
 * Names the project of one directory. The name is prefilled with the app's guess: the repository
 * name from git, or from Superset for a deleted worktree, else the folder name.
 */
function AddRuleDialog(props: { readonly cwd: string; readonly onClose: () => void }): ReactNode {
  const suggestion = useProjectRuleFor(props.cwd);
  const [typed, setTyped] = useState<string | null>(null);
  const addRule = useDataAction((ds, name: string) => ds.addProjectRule(props.cwd, name), {
    failure: "Could not add the project rule",
    success: (_, name) => `Sessions in this directory now belong to ${name}`,
  });
  const name = typed ?? suggestion.data?.name ?? "";
  return (
    <FormDialog
      title="Add project rule"
      isOpen
      onOpenChange={(open) => {
        if (!open) {
          props.onClose();
        }
      }}
      submitLabel="Add rule"
      isPending={addRule.isPending}
      onSubmit={() => addRule.mutate(name.trim(), { onSuccess: props.onClose })}
    >
      <p className="text-sm break-all text-fg-muted">
        <code>{props.cwd}</code>
      </p>
      {suggestion.isPending ? (
        <p role="status" className="text-sm text-fg-muted">
          Looking up the repository name…
        </p>
      ) : (
        <FormTextField
          label="Project name"
          name="name"
          value={name}
          onChange={setTyped}
          isRequired
          autoFocus
          pattern=".*\S.*"
          description="Usage from this directory is grouped under this name, saved as a rule in the config file."
        />
      )}
    </FormDialog>
  );
}

function UnresolvedTable({ rows }: { readonly rows: readonly UnresolvedCwd[] }): ReactNode {
  const { timeZone } = useAppConfig();
  const [editing, setEditing] = useState<string | null>(null);
  const columns: Column<UnresolvedCwd>[] = [
    { header: "Working directory", cell: (row) => <code>{row.cwd}</code> },
    { header: "Events", numeric: true, cell: (row) => formatCount(row.events) },
    {
      header: "API value",
      numeric: true,
      cell: (row) => <Amount micros={row.api_value_usd_micros} currency="USD" />,
    },
    {
      header: "Last activity",
      cell: (row) => formatDateTime(Date.parse(row.last_at), timeZone),
    },
    {
      header: "Rule",
      cell: (row) => (
        <Button onPress={() => setEditing(row.cwd)} aria-label={`Add rule for ${row.cwd}`}>
          Add rule
        </Button>
      ),
    },
  ];
  return (
    <Card title="Unresolved directories">
      <p className="text-sm text-fg-muted">
        These working directories match no project rule, so their usage has no project. Values are
        in USD.
      </p>
      <DataTable
        caption="Unresolved working directories"
        rows={rows}
        rowKey={(row) => row.cwd}
        columns={columns}
      />
      {editing === null ? null : <AddRuleDialog cwd={editing} onClose={() => setEditing(null)} />}
    </Card>
  );
}

/** Hidden when every directory resolves; a failed read still shows, with Retry. */
function UnresolvedCard(): ReactNode {
  const { filters } = useFilters();
  return (
    <CardRemote title="Unresolved directories" remote={toRemote(useUnresolvedCwds(filters))}>
      {(rows) => (rows.length === 0 ? null : <UnresolvedTable rows={rows} />)}
    </CardRemote>
  );
}

/** Projects: cost per project grouped by client, branch drilldown, unresolved cwds. */
export function ProjectsScreen(): ReactNode {
  const { filters } = useFilters();
  const navigate = useNavigate();
  const remote = toRemote(useProjects(filters));
  const open = (project: string): void => {
    void navigate({
      to: "/projects/$project",
      params: { project },
      search: (previous: Record<string, unknown>) => previous,
    });
  };
  return (
    <>
      <PageHeader to="/projects" />
      <RemoteView
        remote={remote}
        emptyWhen={({ rows }) => (rows.length === 0 ? NO_USAGE_COPY : null)}
      >
        {({ rows, currency }) => (
          <>
            {groupByClient(rows).map(([client, inClient]) => (
              <Card key={client} title={client}>
                <ProjectBars
                  rows={inClient}
                  currency={currency}
                  label={`${client} projects by API value`}
                  onOpen={open}
                />
              </Card>
            ))}
            <UnresolvedCard />
          </>
        )}
      </RemoteView>
    </>
  );
}
