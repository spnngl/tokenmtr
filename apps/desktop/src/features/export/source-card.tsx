import type { ReactNode } from "react";
import { formatCount } from "../../shared/format";
import type { SourceHealth } from "../../shared/schemas";
import { Badge, type Column, DataTable, type Tone } from "../../shared/ui";
import { formatInstant, sourceRunLabel } from "./model";

interface SourceCardProps {
  readonly source: SourceHealth;
  readonly timeZone: string;
}

type SourceRun = SourceHealth["recent_runs"][number];

function stateBadge(source: SourceHealth): { readonly tone: Tone; readonly label: string } {
  switch (source.state) {
    case "healthy":
      return { tone: "success", label: "Healthy" };
    case "failing":
      return { tone: "danger", label: "Failing" };
    case "disabled":
      return { tone: "neutral", label: "Disabled" };
    default:
      return source satisfies never;
  }
}

function runColumns(timeZone: string): readonly Column<SourceRun>[] {
  return [
    { header: "Scope", cell: (run) => sourceRunLabel(run) },
    { header: "Finished", cell: (run) => formatInstant(run.finished_at, timeZone) },
    { header: "Rows", numeric: true, cell: (run) => formatCount(run.rows) },
    {
      header: "Result",
      cell: (run) =>
        run.error === null ? (
          <Badge tone="success">OK</Badge>
        ) : (
          <span className="flex flex-col items-start gap-1">
            <Badge tone="danger">Failed</Badge>
            <span className="text-xs">{run.error}</span>
          </span>
        ),
    },
  ];
}

/** One `[[sources]]` entry: its state, what it holds, its last success and its run history. */
export function SourceCard({ source, timeZone }: SourceCardProps): ReactNode {
  const badge = stateBadge(source);
  return (
    <li className="flex flex-col gap-3 rounded-md border border-border bg-surface-raised p-3">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold">{source.source}</h3>
        <Badge tone="neutral">{source.kind === "sqlite" ? "SQLite export" : "Superset"}</Badge>
        <Badge tone={badge.tone}>{badge.label}</Badge>
      </div>
      {source.kind === "superset" ? (
        <p className="text-sm text-fg-muted">
          Names projects after the Superset workspaces your agents ran in.
        </p>
      ) : (
        <dl className="grid grid-cols-1 gap-2 text-sm sm:grid-cols-3">
          <div className="flex flex-col">
            <dt className="text-xs text-fg-muted">Imported</dt>
            <dd className="tabular-nums">
              {formatCount(source.events)} events, {formatCount(source.tool_calls)} tool calls
            </dd>
          </div>
          <div className="flex flex-col">
            <dt className="text-xs text-fg-muted">From</dt>
            <dd className="tabular-nums">
              {formatCount(source.files)} {source.files === 1 ? "file" : "files"},{" "}
              {formatCount(source.machines)} {source.machines === 1 ? "machine" : "machines"}
            </dd>
          </div>
          <div className="flex flex-col">
            <dt className="text-xs text-fg-muted">Last success</dt>
            <dd>
              {source.last_success_at === null
                ? "Never"
                : formatInstant(source.last_success_at, timeZone)}
            </dd>
          </div>
        </dl>
      )}
      {source.state === "failing" ? (
        <p className="text-sm text-danger">
          Failing since {formatInstant(source.since, timeZone)}: {source.last_error}
        </p>
      ) : null}
      {source.kind === "sqlite" && source.recent_runs.length > 0 ? (
        <DataTable
          caption={`Run history of ${source.source}`}
          columns={runColumns(timeZone)}
          rows={source.recent_runs}
          rowKey={(run) => `${run.path ?? ""}/${run.started_at}`}
        />
      ) : null}
      {source.state === "disabled" ? (
        <p className="text-sm text-fg-muted">
          This source is switched off in the config; its imported rows still count.
        </p>
      ) : null}
    </li>
  );
}
