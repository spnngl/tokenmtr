import { Link } from "@tanstack/react-router";
import { ArrowLeft, Check, Copy, FolderOpen } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { useDataAction } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { formatDateTime, formatTokens } from "../../shared/format";
import { queryViewState, useSessionDetail, useSessionOrigin } from "../../shared/queries";
import type { SessionEvent, SessionOrigin, SessionToolCall } from "../../shared/schemas";
import { StateView } from "../../shared/state-view";
import { Amount, Card, type Column, DataTable, PageHeader } from "../../shared/ui";
import { buildResumeCommand } from "./resume-command";

const COST_SOURCE_LABEL = {
  reported: "Reported",
  estimated: "Estimated",
  unpriced: "Unpriced",
} as const;

function dirname(path: string): string {
  const cut = path.lastIndexOf("/");
  return cut > 0 ? path.slice(0, cut) : path;
}

function CopyButton({ text }: { readonly text: string }): ReactNode {
  const [copied, setCopied] = useState(false);
  const copy = (): void => {
    void navigator.clipboard.writeText(text).then(() => setCopied(true));
  };
  return (
    <Button variant="secondary" onPress={copy}>
      {copied ? <Check className="size-4" aria-hidden /> : <Copy className="size-4" aria-hidden />}
      {copied ? "Copied" : "Copy command"}
    </Button>
  );
}

function ResumeCard(props: {
  readonly agent: string;
  readonly session: string;
  readonly origin: SessionOrigin;
}): ReactNode {
  const { origin } = props;
  const reveal = useDataAction((ds, path: string) => ds.revealPath(path));
  const command = buildResumeCommand(props.agent, props.session, origin);
  const { transcript_path: transcript } = origin;
  return (
    <Card title="Resume">
      {command.kind === "command" ? (
        <>
          <pre className="min-w-0 overflow-x-auto rounded-md bg-surface-muted p-3 text-sm">
            <code>{command.text}</code>
          </pre>
          <div className="flex flex-wrap gap-2">
            <CopyButton text={command.text} />
          </div>
        </>
      ) : (
        <p className="text-sm text-fg-muted">{command.reason}</p>
      )}
      {origin.transcript_exists && transcript !== null ? (
        <div className="flex flex-wrap gap-2">
          <Button onPress={() => reveal.mutate(transcript)}>Reveal transcript</Button>
          <Button onPress={() => reveal.mutate(dirname(transcript))}>
            <FolderOpen className="size-4" aria-hidden />
            Open folder
          </Button>
        </div>
      ) : (
        <p className="text-sm text-fg-muted">
          The transcript file is gone (the agent cleaned it up), so only the recorded usage remains.
        </p>
      )}
      {reveal.error === null ? null : (
        <p role="alert" className="text-sm text-danger">
          {reveal.error.message}
        </p>
      )}
    </Card>
  );
}

function OriginSection(props: { readonly agent: string; readonly session: string }): ReactNode {
  const query = useSessionOrigin(props.agent, props.session);
  if (query.data === undefined) {
    return null;
  }
  return <ResumeCard agent={props.agent} session={props.session} origin={query.data} />;
}

/** Drilldown of one session (PLAN §7): resume command, per-event rows, tool calls, signals. */
export function SessionDetailScreen(props: {
  readonly agent: string;
  readonly session: string;
}): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  const query = useSessionDetail(props.agent, props.session, filters.currency);
  const state = queryViewState(query, (detail) =>
    detail.events.length === 0 && detail.tool_calls.length === 0
      ? { title: "Session not found", hint: "No recorded events for this session." }
      : null,
  );
  const detail = query.data;
  const at = (iso: string): string => formatDateTime(Date.parse(iso), timeZone);

  const eventColumns: Column<SessionEvent>[] = [
    { id: "at", header: "Time", cell: (row) => at(row.at), sortValue: (row) => row.at },
    { id: "model", header: "Model", cell: (row) => row.model, sortValue: (row) => row.model },
    { id: "kind", header: "Kind", cell: (row) => row.kind },
    { id: "branch", header: "Branch", cell: (row) => row.git_branch ?? "—" },
    {
      id: "tokens",
      header: "Tokens",
      align: "end",
      cell: (row) => formatTokens(row.total_tokens),
      sortValue: (row) => row.total_tokens,
    },
    { id: "output", header: "Output", align: "end", cell: (row) => formatTokens(row.output) },
    {
      id: "value",
      header: "API value",
      align: "end",
      cell: (row) => (
        <Amount
          micros={row.api_value_micros}
          currency={filters.currency}
          approximate={row.cost_source !== "reported"}
        />
      ),
      sortValue: (row) => row.api_value_micros,
    },
    { id: "source", header: "Cost source", cell: (row) => COST_SOURCE_LABEL[row.cost_source] },
  ];
  const toolColumns: Column<SessionToolCall>[] = [
    { id: "at", header: "Time", cell: (row) => at(row.at), sortValue: (row) => row.at },
    { id: "tool", header: "Tool", cell: (row) => row.tool_name, sortValue: (row) => row.tool_name },
    { id: "server", header: "MCP server", cell: (row) => row.mcp_server ?? "—" },
    {
      id: "status",
      header: "Result",
      cell: (row) => {
        if (row.is_error === null) {
          return "Unknown";
        }
        return row.is_error ? "Failed" : "OK";
      },
    },
  ];

  return (
    <>
      <Link
        to="/sessions"
        search={(previous: Record<string, unknown>) => previous}
        className="inline-flex w-fit items-center gap-1 text-sm font-medium text-accent"
      >
        <ArrowLeft className="size-4" aria-hidden />
        All sessions
      </Link>
      <PageHeader title={`Session ${props.session}`} summary={`Agent: ${props.agent}`} />
      <StateView state={state}>
        <OriginSection agent={props.agent} session={props.session} />
        {detail === undefined ? null : (
          <>
            {detail.signals.length === 0 ? null : (
              <Card title="Signals">
                <ul className="flex flex-col gap-1 text-sm">
                  {detail.signals.map(([when, kind]) => (
                    <li key={`${when}:${kind}`}>
                      {at(when)}: {kind === "api_error" ? "API error" : "Context compaction"}
                    </li>
                  ))}
                </ul>
              </Card>
            )}
            <Card title="Events">
              {detail.events_truncated ? (
                <p role="status" className="text-sm text-fg-muted">
                  Only the first events of this session are shown.
                </p>
              ) : null}
              <DataTable
                caption="Session events"
                rows={detail.events}
                rowKey={(row, index) => `${index}:${row.at}`}
                columns={eventColumns}
              />
            </Card>
            <Card title="Tool calls">
              {detail.tool_calls.length === 0 ? (
                <p className="text-sm text-fg-muted">No tool calls recorded.</p>
              ) : (
                <DataTable
                  caption="Tool call list"
                  rows={detail.tool_calls}
                  rowKey={(row, index) => `${index}:${row.at}`}
                  columns={toolColumns}
                />
              )}
            </Card>
          </>
        )}
      </StateView>
    </>
  );
}
