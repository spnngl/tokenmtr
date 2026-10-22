import { Link } from "@tanstack/react-router";
import { type ReactNode, useEffect, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { useFilters } from "../../app/use-filters";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { serializeFilters } from "../../shared/filters";
import {
  formatCount,
  formatDateTime,
  formatDuration,
  formatTokens,
  PLACEHOLDER,
} from "../../shared/format";
import { useSessions } from "../../shared/queries";
import { RemoteView } from "../../shared/remote-view";
import type { SessionPage, SessionRow } from "../../shared/schemas";
import { Amount, type Column, DataTable, PageHeader } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";
import { SessionAnalysisPanel } from "./session-analysis";

/** Rows asked per page; the answer's `page_size` is what the pager counts with. */
const PAGE_SIZE = 25;

/** Number of pages of `data`, at least one. */
export function pageCount(data: Pick<SessionPage, "total" | "page_size">): number {
  return Math.max(1, Math.ceil(data.total / Math.max(1, data.page_size)));
}

function columns(
  timeZone: string,
  currency: SessionPage["currency"],
  withMachine: boolean,
): Column<SessionRow>[] {
  const machine: Column<SessionRow>[] = withMachine
    ? [
        {
          header: "Machine",
          cell: (row) => row.machine ?? "This machine",
          sortValue: (row) => row.machine ?? "",
        },
      ]
    : [];
  return [
    {
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
    { header: "Agent", cell: (row) => row.agent, sortValue: (row) => row.agent },
    {
      header: "Started",
      cell: (row) => formatDateTime(Date.parse(row.started_at), timeZone),
      sortValue: (row) => row.started_at,
    },
    {
      header: "Duration",
      numeric: true,
      cell: (row) => formatDuration(row.duration_secs),
      sortValue: (row) => row.duration_secs,
    },
    {
      header: "Project",
      cell: (row) => row.project ?? PLACEHOLDER,
      sortValue: (row) => row.project ?? "",
    },
    ...machine,
    { header: "Models", cell: (row) => row.models.join(", ") },
    {
      header: "Tokens",
      numeric: true,
      cell: (row) => formatTokens(row.total_tokens),
      sortValue: (row) => row.total_tokens,
    },
    {
      header: "API value",
      numeric: true,
      cell: (row) => (
        <Amount micros={row.api_value_micros} currency={currency} approximate={row.approximate} />
      ),
      sortValue: (row) => row.api_value_micros,
    },
    {
      header: "Paid",
      numeric: true,
      cell: (row) => <Amount micros={row.paid_micros} currency={currency} />,
      sortValue: (row) => row.paid_micros,
    },
    {
      header: "Tool calls",
      numeric: true,
      cell: (row) =>
        row.tool_errors === 0
          ? formatCount(row.tool_calls)
          : `${formatCount(row.tool_calls)} (${formatCount(row.tool_errors)} failed)`,
      sortValue: (row) => row.tool_calls,
    },
  ];
}

function Pager(props: {
  readonly data: SessionPage;
  readonly onPage: (page: number) => void;
}): ReactNode {
  const { page, total } = props.data;
  const pages = pageCount(props.data);
  return (
    <nav aria-label="Pagination" className="flex items-center justify-between gap-3 text-sm">
      <span className="text-fg-muted">
        Page {page + 1} of {pages} · {formatCount(total)} sessions
      </span>
      <span className="flex gap-2">
        <Button isDisabled={page === 0} onPress={() => props.onPage(page - 1)}>
          Previous
        </Button>
        <Button isDisabled={page + 1 >= pages} onPress={() => props.onPage(page + 1)}>
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
  const data = query.data;
  // A store change can shrink the list under the current page: fall back to its last page.
  useEffect(() => {
    if (data !== undefined && data.page === page && page >= pageCount(data)) {
      setPage(pageCount(data) - 1);
    }
  }, [data, page]);
  return (
    <RemoteView
      remote={toRemote(query)}
      emptyWhen={(answer) =>
        answer.total === 0
          ? {
              title: "No sessions",
              hint: "No session matches these filters. Widen the range or clear a filter.",
            }
          : null
      }
    >
      {(answer) => (
        <>
          <DataTable
            caption="Sessions, newest first"
            rows={answer.rows}
            rowKey={(row) => `${row.agent}:${row.session}`}
            columns={columns(
              timeZone,
              answer.currency,
              answer.rows.some((row) => row.machine !== null),
            )}
          />
          <Pager data={answer} onPage={setPage} />
        </>
      )}
    </RemoteView>
  );
}

const TAB_CLASS =
  "-mb-px min-h-9 cursor-pointer border-b-2 border-transparent px-3 text-sm font-medium text-fg-muted data-hovered:text-fg data-selected:border-accent data-selected:text-fg";

/**
 * Sessions: a server-paged table (newest first; sorting reorders the loaded page) and, on its own
 * tab, charts over every session of the filters.
 */
export function SessionsScreen(): ReactNode {
  const { filters } = useFilters();
  return (
    <>
      <PageHeader to="/sessions" />
      <Tabs defaultSelectedKey="table" className="flex flex-col gap-4">
        <TabList aria-label="Sessions view" className="flex gap-1 border-b border-border">
          <Tab id="table" className={TAB_CLASS}>
            Table
          </Tab>
          <Tab id="analysis" className={TAB_CLASS}>
            Analysis
          </Tab>
        </TabList>
        <TabPanel id="table" className="flex flex-col gap-4 outline-offset-4">
          {/* Keyed by the filters: a new filter set starts again at the first page. */}
          <SessionsTable key={serializeFilters(filters).toString()} />
        </TabPanel>
        <TabPanel id="analysis" className="outline-offset-4">
          <SessionAnalysisPanel />
        </TabPanel>
      </Tabs>
    </>
  );
}
