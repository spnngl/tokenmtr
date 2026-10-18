import { Link } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { serializeFilters } from "../../shared/filters";
import { formatDateTime, formatDuration, formatTokens } from "../../shared/format";
import { queryViewState, useSessions } from "../../shared/queries";
import type { SessionPage, SessionRow } from "../../shared/schemas";
import { StateView } from "../../shared/state-view";
import { Amount, type Column, DataTable, PageHeader } from "../../shared/ui";

const PAGE_SIZE = 25;

function columns(timeZone: string, currency: SessionPage["currency"]): Column<SessionRow>[] {
  return [
    {
      id: "session",
      header: "Session",
      cell: (row) => (
        <Link
          to="/sessions/$agent/$session"
          params={{ agent: row.agent, session: row.session }}
          search={(previous: Record<string, unknown>) => previous}
          className="font-medium text-accent underline"
        >
          {row.session.slice(0, 8)}
          <span className="sr-only"> {row.session.slice(8)}</span>
        </Link>
      ),
    },
    { id: "agent", header: "Agent", cell: (row) => row.agent, sortValue: (row) => row.agent },
    {
      id: "started",
      header: "Started",
      cell: (row) => formatDateTime(Date.parse(row.started_at), timeZone),
      sortValue: (row) => row.started_at,
    },
    {
      id: "duration",
      header: "Duration",
      align: "end",
      cell: (row) => formatDuration(row.duration_secs),
      sortValue: (row) => row.duration_secs,
    },
    {
      id: "project",
      header: "Project",
      cell: (row) => row.project ?? "—",
      sortValue: (row) => row.project ?? "",
    },
    { id: "models", header: "Models", cell: (row) => row.models.join(", ") },
    {
      id: "tokens",
      header: "Tokens",
      align: "end",
      cell: (row) => formatTokens(row.total_tokens),
      sortValue: (row) => row.total_tokens,
    },
    {
      id: "value",
      header: "API value",
      align: "end",
      cell: (row) => (
        <Amount micros={row.api_value_micros} currency={currency} approximate={row.approximate} />
      ),
      sortValue: (row) => row.api_value_micros,
    },
    {
      id: "paid",
      header: "Paid",
      align: "end",
      cell: (row) => <Amount micros={row.paid_micros} currency={currency} />,
      sortValue: (row) => row.paid_micros,
    },
    {
      id: "tools",
      header: "Tool calls",
      align: "end",
      cell: (row) =>
        row.tool_errors === 0
          ? String(row.tool_calls)
          : `${row.tool_calls} (${row.tool_errors} failed)`,
      sortValue: (row) => row.tool_calls,
    },
  ];
}

function Pager(props: {
  readonly page: number;
  readonly total: number;
  readonly onPage: (page: number) => void;
}): ReactNode {
  const pages = Math.max(1, Math.ceil(props.total / PAGE_SIZE));
  return (
    <nav aria-label="Pagination" className="flex items-center justify-between gap-3 text-sm">
      <span className="text-fg-muted">
        Page {props.page + 1} of {pages} · {props.total.toLocaleString("en-US")} sessions
      </span>
      <span className="flex gap-2">
        <Button isDisabled={props.page === 0} onPress={() => props.onPage(props.page - 1)}>
          Previous
        </Button>
        <Button isDisabled={props.page + 1 >= pages} onPress={() => props.onPage(props.page + 1)}>
          Next
        </Button>
      </span>
    </nav>
  );
}

function SessionsTable(): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  const [page, setPage] = useState(0);
  const query = useSessions(filters, page, PAGE_SIZE);
  const state = queryViewState(query, (data) =>
    data.total === 0
      ? {
          title: "No sessions",
          hint: "No session matches these filters. Widen the range or clear a filter.",
        }
      : null,
  );
  const data = query.data;
  return (
    <StateView state={state}>
      {data === undefined ? null : (
        <>
          <DataTable
            caption="Sessions, newest first"
            rows={data.rows}
            rowKey={(row) => `${row.agent}:${row.session}`}
            columns={columns(timeZone, data.currency)}
          />
          <Pager page={page} total={data.total} onPage={setPage} />
        </>
      )}
    </StateView>
  );
}

/** Sessions (PLAN §7): server-paged, newest first; sorting reorders the loaded page. */
export function SessionsScreen(): ReactNode {
  const { filters } = useFilters();
  return (
    <>
      <PageHeader
        title="Sessions"
        summary="Every session with tokens, cost and duration, and a drilldown per event."
      />
      {/* Keyed by the filters: a new filter set starts again at the first page. */}
      <SessionsTable key={serializeFilters(filters).toString()} />
    </>
  );
}
