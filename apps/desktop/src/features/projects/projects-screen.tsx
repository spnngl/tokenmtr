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
  useModels,
  useProjectModels,
  useProjectRemotes,
  useProjectRuleFor,
  useProjects,
  useSeries,
  useUnresolvedCwds,
} from "../../shared/queries";
import { RankedBars } from "../../shared/ranked-bars";
import { CardRemote, NO_USAGE_COPY, RemoteView } from "../../shared/remote-view";
import type {
  ProjectGrouping,
  ProjectRemote,
  ProjectRow,
  UnresolvedCwd,
} from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
import { Amount, Card, type Column, DataTable, PageHeader } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";
import { CostByModelPanel } from "../insights/cost-by-model-panel";
import {
  ModelCostBars,
  ProjectCostLines,
  ProjectModelBars,
  projectLabel,
  seriesLabels,
} from "./project-charts";

const NO_CLIENT = "No client";

const GROUPINGS: readonly { readonly value: ProjectGrouping; readonly label: string }[] = [
  { value: "project", label: "Remote" },
  { value: "folder", label: "Folder" },
];

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
  /** Absent for folder rows: a folder has no detail page. */
  readonly onOpen: ((project: string) => void) | null;
}): ReactNode {
  const { onOpen } = props;
  return (
    <RankedBars
      label={props.label}
      items={props.rows.map((row) => {
        const { project } = row;
        const share = `${formatPercent(row.share_percent / 100)} of cost · ${formatTokens(row.total_tokens)} tokens`;
        return {
          key: project ?? "(none)",
          label: projectLabel(row),
          value: (
            <Amount
              micros={row.api_value_micros}
              currency={props.currency}
              approximate={row.approximate}
            />
          ),
          percent: row.share_percent,
          // A folder's name is its last component: the path tells two clones apart.
          detail: onOpen === null && project !== null ? `${project} · ${share}` : share,
          ...(project === null || onOpen === null ? {} : { onPress: () => onOpen(project) }),
        };
      })}
    />
  );
}

/** Cost over time per project (or folder), and each one's split by model. */
function ProjectsCharts(props: {
  readonly rows: readonly ProjectRow[];
  readonly grouping: ProjectGrouping;
}): ReactNode {
  const { filters } = useFilters();
  const { rows, grouping } = props;
  const series = useSeries(filters, grouping);
  const split = useProjectModels(filters, grouping);
  return (
    <>
      <CardRemote title="Cost over time" remote={toRemote(series)}>
        {(points) => <ProjectCostLines series={points} label={seriesLabels(rows)} />}
      </CardRemote>
      <CardRemote
        title={grouping === "folder" ? "Cost by model per folder" : "Cost by model per project"}
        remote={toRemote(split)}
      >
        {(models) => <ProjectModelBars split={models} grouping={grouping} />}
      </CardRemote>
    </>
  );
}

/** One project's cost by model, overall and over time. */
function ProjectDetailCharts({ project }: { readonly project: string }): ReactNode {
  const { filters } = useFilters();
  const scoped = { ...filters, project };
  return (
    <>
      <CardRemote title="Cost by model" remote={toRemote(useModels(scoped))}>
        {({ rows, currency }) => <ModelCostBars rows={rows} currency={currency} />}
      </CardRemote>
      <CardRemote title="Cost by model over time" remote={toRemote(useSeries(scoped, "model"))}>
        {(series) => <CostByModelPanel costs={series} />}
      </CardRemote>
    </>
  );
}

/** One project's models and branches (`/projects/$project`); the filter bar's filters still apply. */
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
      <PageHeader title={project} summary="Cost per model and per git branch." />
      <RemoteView
        remote={remote}
        emptyWhen={({ rows }) => (rows.length === 0 ? NO_USAGE_COPY : null)}
      >
        {({ rows, currency }) => {
          const max = Math.max(1, ...rows.map((row) => row.api_value_micros));
          return (
            <>
              <ProjectDetailCharts project={project} />
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
            </>
          );
        }}
      </RemoteView>
      <CardRemote title="Remotes" remote={toRemote(useProjectRemotes(project))}>
        {(rows) => <RemotesCard rows={rows} />}
      </CardRemote>
    </>
  );
}

const REMOTE_COLUMNS: Column<ProjectRemote>[] = [
  {
    header: "Remote",
    cell: (row) => (row.keys_project ? `${row.name} (project key)` : row.name),
  },
  { header: "Repository", cell: (row) => row.repository },
  { header: "Default branch", cell: (row) => row.default_branch ?? "Unknown" },
];

/** The git remotes of the project's repositories, read from the clones without network. */
function RemotesCard({ rows }: { readonly rows: readonly ProjectRemote[] }): ReactNode {
  return (
    <Card title="Remotes">
      {rows.length === 0 ? (
        <p className="text-sm text-fg-muted">
          No git remote: this project is not a repository with a shared remote, or its paths are
          redacted.
        </p>
      ) : (
        <DataTable
          caption="Git remotes"
          rows={rows}
          rowKey={(row) => `${row.name} ${row.host}/${row.repository}`}
          columns={REMOTE_COLUMNS}
        />
      )}
    </Card>
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

/**
 * Projects: cost per project grouped by client, branch drilldown, unresolved cwds. "Group by"
 * switches between project keys (the git remote) and folders (repository root, else cwd) for this
 * view only.
 */
export function ProjectsScreen(): ReactNode {
  const { filters } = useFilters();
  const navigate = useNavigate();
  const [grouping, setGrouping] = useState<ProjectGrouping>("project");
  const remote = toRemote(useProjects(filters, grouping));
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
      <div className="flex items-center gap-2">
        <span className="text-sm text-fg-muted" aria-hidden>
          Group by
        </span>
        <SegmentedControl
          label="Group projects by"
          options={GROUPINGS}
          value={grouping}
          onChange={setGrouping}
        />
      </div>
      <RemoteView
        remote={remote}
        emptyWhen={({ rows }) => (rows.length === 0 ? NO_USAGE_COPY : null)}
      >
        {({ rows, currency }) => (
          <>
            <ProjectsCharts rows={rows} grouping={grouping} />
            {groupByClient(rows).map(([client, inClient]) => (
              <Card key={client} title={client}>
                <ProjectBars
                  rows={inClient}
                  currency={currency}
                  label={`${client} ${grouping === "folder" ? "folders" : "projects"} by API value`}
                  onOpen={grouping === "folder" ? null : open}
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
