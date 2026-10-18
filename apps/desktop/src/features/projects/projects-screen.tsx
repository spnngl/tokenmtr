import { ArrowLeft } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { useDataAction } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { formatDateTime, formatTokens } from "../../shared/format";
import {
  NO_USAGE_COPY,
  queryViewState,
  useBranches,
  useProjects,
  useUnresolvedCwds,
} from "../../shared/queries";
import { RankedBars } from "../../shared/ranked-bars";
import type { ProjectRow } from "../../shared/schemas";
import { StateView } from "../../shared/state-view";
import { Amount, Card, type Column, DataTable, PageHeader } from "../../shared/ui";

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
  readonly label: string;
  readonly onOpen: (project: string) => void;
}): ReactNode {
  const { filters } = useFilters();
  return (
    <RankedBars
      label={props.label}
      items={props.rows.map((row) => ({
        key: row.project ?? "(none)",
        label: row.name ?? row.project ?? "No project",
        value: (
          <Amount
            micros={row.api_value_micros}
            currency={filters.currency}
            approximate={row.approximate}
          />
        ),
        percent: row.share_percent,
        detail: `${row.share_percent.toFixed(0)}% of cost · ${formatTokens(row.total_tokens)} tokens`,
        ...(row.project === null ? {} : { onPress: () => props.onOpen(row.project ?? "") }),
      }))}
    />
  );
}

function ProjectDetail(props: {
  readonly project: string;
  readonly onBack: () => void;
}): ReactNode {
  const { filters } = useFilters();
  const query = useBranches({ ...filters, project: props.project });
  const state = queryViewState(query, (rows) => (rows.length === 0 ? NO_USAGE_COPY : null));
  const rows = query.data ?? [];
  const max = Math.max(1, ...rows.map((row) => row.api_value_micros));
  return (
    <>
      <div>
        <Button variant="quiet" onPress={props.onBack}>
          <ArrowLeft className="size-4" aria-hidden />
          All projects
        </Button>
      </div>
      <PageHeader title={props.project} summary="Cost per git branch." />
      <StateView state={state}>
        <Card title="Branches">
          <RankedBars
            label="Branches by API value"
            items={rows.map((row) => ({
              key: row.branch ?? "(none)",
              label: row.branch ?? "No branch",
              value: (
                <Amount
                  micros={row.api_value_micros}
                  currency={filters.currency}
                  approximate={row.approximate}
                />
              ),
              percent: (row.api_value_micros / max) * 100,
              detail: `${formatTokens(row.total_tokens)} tokens`,
            }))}
          />
        </Card>
      </StateView>
    </>
  );
}

function UnresolvedCard(): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  const rows = useUnresolvedCwds(filters).data ?? [];
  const addRule = useDataAction((ds, cwd: string) => ds.addProjectRule(cwd));
  if (rows.length === 0) {
    return null;
  }
  const columns: Column<(typeof rows)[number]>[] = [
    { id: "cwd", header: "Working directory", cell: (row) => <code>{row.cwd}</code> },
    {
      id: "events",
      header: "Events",
      align: "end",
      cell: (row) => row.events.toLocaleString("en-US"),
    },
    {
      id: "value",
      header: "API value",
      align: "end",
      cell: (row) => <Amount micros={row.api_value_usd_micros} currency="USD" />,
    },
    {
      id: "last",
      header: "Last activity",
      cell: (row) => formatDateTime(Date.parse(row.last_at), timeZone),
    },
    {
      id: "rule",
      header: "Rule",
      cell: (row) => (
        <Button onPress={() => addRule.mutate(row.cwd)} aria-label={`Add rule for ${row.cwd}`}>
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
      {addRule.error === null ? null : (
        <p role="alert" className="text-sm text-danger">
          {addRule.error.message}
        </p>
      )}
    </Card>
  );
}

function ProjectList(props: { readonly onOpen: (project: string) => void }): ReactNode {
  const { filters } = useFilters();
  const query = useProjects(filters);
  const state = queryViewState(query, (rows) => (rows.length === 0 ? NO_USAGE_COPY : null));
  const groups = groupByClient(query.data ?? []);
  return (
    <StateView state={state}>
      {groups.map(([client, rows]) => (
        <Card key={client} title={client}>
          <ProjectBars
            rows={rows}
            label={`${client} projects by API value`}
            onOpen={props.onOpen}
          />
        </Card>
      ))}
      <UnresolvedCard />
    </StateView>
  );
}

/** Projects (PLAN §7): cost per project grouped by client, branch drilldown, unresolved cwds. */
export function ProjectsScreen(): ReactNode {
  const [selected, setSelected] = useState<string | null>(null);
  if (selected !== null) {
    return <ProjectDetail project={selected} onBack={() => setSelected(null)} />;
  }
  return (
    <>
      <PageHeader
        title="Projects"
        summary="Cost per project and client, with a branch drilldown."
      />
      <ProjectList onOpen={setSelected} />
    </>
  );
}
