import { Link } from "@tanstack/react-router";
import { ArrowLeft, Check, Copy, FolderOpen } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { useRevealPath } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { errorText } from "../../shared/errors";
import { type Currency, formatDateTime, formatTokens, PLACEHOLDER } from "../../shared/format";
import { useSessionDetail, useSessionOrigin } from "../../shared/queries";
import { CardRemote, RemoteView } from "../../shared/remote-view";
import type { EventKind, SessionEvent, SessionOrigin, SessionToolCall } from "../../shared/schemas";
import { useNotify } from "../../shared/toasts";
import { Amount, Card, type Column, DataTable, PageHeader } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";
import { buildResumeCommand } from "./resume-command";
import { SessionChartsPanel } from "./session-charts";

const COST_SOURCE_LABEL = {
  reported: "Reported",
  estimated: "Estimated",
  unpriced: "Unpriced",
} as const;

const EVENT_KIND_LABEL: Readonly<Record<EventKind, string>> = {
  message: "Message",
  tool_result: "Tool result",
  compaction: "Compaction",
  branch_summary: "Branch summary",
  auxiliary: "Auxiliary call",
  daily_aggregate: "Daily total",
  extra_usage: "Extra usage",
};

/** How long the button says "Copied" before it offers to copy again. */
const COPIED_MS = 2_000;

function CopyButton({ text }: { readonly text: string }): ReactNode {
  const notify = useNotify();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = setTimeout(() => setCopied(false), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copied]);
  const copy = (): void => {
    navigator.clipboard.writeText(text).then(
      () => setCopied(true),
      (error: unknown) =>
        notify({
          tone: "error",
          title: "Could not copy the command",
          message: errorText(error).message,
        }),
    );
  };
  return (
    <>
      <Button variant="secondary" onPress={copy}>
        {copied ? (
          <Check className="size-4" aria-hidden />
        ) : (
          <Copy className="size-4" aria-hidden />
        )}
        {copied ? "Copied" : "Copy command"}
      </Button>
      <span role="status" className="sr-only">
        {copied ? "Resume command copied" : ""}
      </span>
    </>
  );
}

function ResumeCard(props: {
  readonly agent: string;
  readonly session: string;
  readonly origin: SessionOrigin;
}): ReactNode {
  const { origin } = props;
  const { agent, session } = props;
  const reveal = useRevealPath();
  const revealing = (kind: "transcript" | "transcript_folder"): boolean =>
    reveal.isPending && reveal.variables.kind === kind;
  const command = buildResumeCommand(agent, session, origin);
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
      {origin.transcript_exists && origin.transcript_path !== null ? (
        <div className="flex flex-wrap gap-2">
          <Button
            isPending={revealing("transcript")}
            onPress={() => reveal.mutate({ kind: "transcript", agent, session })}
          >
            Reveal transcript
          </Button>
          <Button
            isPending={revealing("transcript_folder")}
            onPress={() => reveal.mutate({ kind: "transcript_folder", agent, session })}
          >
            <FolderOpen className="size-4" aria-hidden />
            Open folder
          </Button>
        </div>
      ) : (
        <p className="text-sm text-fg-muted">
          The transcript file is gone (the agent cleaned it up), so only the recorded usage remains.
        </p>
      )}
    </Card>
  );
}

function OriginSection(props: { readonly agent: string; readonly session: string }): ReactNode {
  const origin = toRemote(useSessionOrigin(props.agent, props.session));
  return (
    <CardRemote title="Resume" remote={origin}>
      {(origin) => <ResumeCard agent={props.agent} session={props.session} origin={origin} />}
    </CardRemote>
  );
}

/** Drilldown of one session: resume command, per-event rows, tool calls, signals. */
export function SessionDetailScreen(props: {
  readonly agent: string;
  readonly session: string;
}): ReactNode {
  const { filters } = useFilters();
  const { timeZone } = useAppConfig();
  const detail = toRemote(useSessionDetail(props.agent, props.session, filters.currency));
  const at = (iso: string): string => formatDateTime(Date.parse(iso), timeZone);

  // Amounts are in the currency the backend answered in (EUR falls back to USD without a rate).
  const eventColumns = (currency: Currency): Column<SessionEvent>[] => [
    { header: "Time", cell: (row) => at(row.at), sortValue: (row) => row.at },
    { header: "Model", cell: (row) => row.model, sortValue: (row) => row.model },
    { header: "Kind", cell: (row) => EVENT_KIND_LABEL[row.kind] },
    { header: "Branch", cell: (row) => row.git_branch ?? PLACEHOLDER },
    {
      header: "Tokens",
      numeric: true,
      cell: (row) => formatTokens(row.total_tokens),
      sortValue: (row) => row.total_tokens,
    },
    { header: "Output", numeric: true, cell: (row) => formatTokens(row.output) },
    {
      header: "API value",
      numeric: true,
      cell: (row) => (
        <Amount
          micros={row.api_value_micros}
          currency={currency}
          approximate={row.cost_source !== "reported"}
        />
      ),
      sortValue: (row) => row.api_value_micros,
    },
    { header: "Cost source", cell: (row) => COST_SOURCE_LABEL[row.cost_source] },
  ];
  const toolColumns: Column<SessionToolCall>[] = [
    { header: "Time", cell: (row) => at(row.at), sortValue: (row) => row.at },
    { header: "Tool", cell: (row) => row.tool_name, sortValue: (row) => row.tool_name },
    { header: "MCP server", cell: (row) => row.mcp_server ?? PLACEHOLDER },
    {
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
      <RemoteView
        remote={detail}
        emptyWhen={(detail) =>
          detail.events.length === 0 && detail.tool_calls.length === 0
            ? { title: "Session not found", hint: "No recorded events for this session." }
            : null
        }
      >
        {(detail) => (
          <>
            <OriginSection agent={props.agent} session={props.session} />
            <SessionChartsPanel
              agent={props.agent}
              session={props.session}
              currency={filters.currency}
              timeZone={timeZone}
            />
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
                columns={eventColumns(detail.currency)}
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
      </RemoteView>
    </>
  );
}
