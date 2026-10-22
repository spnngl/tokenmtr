import type { ReactNode } from "react";
import { formatCount } from "../../shared/format";
import type { SourceHealth } from "../../shared/schemas";
import { Badge, type Column, DataTable, type Tone } from "../../shared/ui";
import { formatInstant } from "./model";
import { resultColumn } from "./result-column";

interface SourceCardProps {
  readonly source: SourceHealth;
  readonly timeZone: string;
}

type SourceRun = SourceHealth["recent_runs"][number];

const STATE_BADGE: Readonly<Record<SourceHealth["state"], { tone: Tone; label: string }>> = {
  healthy: { tone: "success", label: "Healthy" },
  failing: { tone: "danger", label: "Failing" },
  disabled: { tone: "neutral", label: "Disabled" },
};

/** The file a run read, or the whole pass when it names none. */
function scope(run: SourceRun): string {
  return run.path === null ? "All files" : (run.path.split(/[\\/]/).pop() ?? run.path);
}

function runColumns(timeZone: string): readonly Column<SourceRun>[] {
  return [
    { header: "Scope", cell: scope },
    { header: "Finished", cell: (run) => formatInstant(run.finished_at, timeZone) },
    { header: "Rows", numeric: true, cell: (run) => formatCount(run.rows) },
    resultColumn<SourceRun>(),
  ];
}

/** One `sqlite` source: its state, what it holds, its last success and its run history. */
export function SourceCard({ source, timeZone }: SourceCardProps): ReactNode {
  const badge = STATE_BADGE[source.state];
  return (
    <li className="flex flex-col gap-3 rounded-md border border-border bg-surface-raised p-3">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold">{source.source}</h3>
        <Badge tone={badge.tone}>{badge.label}</Badge>
      </div>
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
      {source.state === "failing" ? (
        <p className="text-sm text-danger">
          Failing since {formatInstant(source.since, timeZone)}: {source.last_error}
        </p>
      ) : null}
      {source.recent_runs.length > 0 ? (
        <DataTable
          caption={`Run history of ${source.source}`}
          columns={runColumns(timeZone)}
          rows={source.recent_runs}
          rowKey={(run, index) => `${index}:${run.started_at}`}
        />
      ) : null}
      {source.state === "disabled" ? (
        <p className="text-sm text-fg-muted">
          Switched off in the config; its imported rows still count.
        </p>
      ) : null}
    </li>
  );
}
