import type { ReactNode } from "react";
import { Button } from "../../shared/button";
import { formatCount, formatDuration } from "../../shared/format";
import type { SinkHealth, SinkRun } from "../../shared/schemas";
import { Badge, type Column, DataTable, type Tone } from "../../shared/ui";
import { canExport, formatInstant, looksLikeProtectedFolder } from "./model";

interface SinkCardProps {
  readonly sink: SinkHealth;
  readonly timeZone: string;
  /** Runs an export of every sink; the daemon has no per-sink trigger. */
  readonly onRetry: () => void;
  readonly busy: boolean;
}

function stateBadge(sink: SinkHealth): { readonly tone: Tone; readonly label: string } {
  switch (sink.state) {
    case "healthy":
      return { tone: "success", label: "Healthy" };
    case "lagging":
      return { tone: "warning", label: "Lagging" };
    case "failing":
      return { tone: "danger", label: "Failing" };
    case "disabled":
      return { tone: "neutral", label: "Disabled" };
    default:
      return sink satisfies never;
  }
}

function runColumns(timeZone: string): readonly Column<SinkRun>[] {
  return [
    { header: "Dataset", cell: (run) => run.dataset },
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

/** One sink: state, lag, last success, error and protected-folder hint, and its run history. */
export function SinkCard({ sink, timeZone, onRetry, busy }: SinkCardProps): ReactNode {
  const badge = stateBadge(sink);
  return (
    <li className="flex flex-col gap-3 rounded-md border border-border bg-surface-raised p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="text-sm font-semibold">{sink.sink}</h3>
          <Badge tone={badge.tone}>{badge.label}</Badge>
        </div>
        {sink.state === "failing" ? (
          <Button
            variant="secondary"
            isPending={busy}
            aria-label={`Retry export of ${sink.sink}`}
            onPress={onRetry}
          >
            Retry
          </Button>
        ) : null}
      </div>
      <dl className="grid grid-cols-1 gap-2 text-sm sm:grid-cols-3">
        <div className="flex flex-col">
          <dt className="text-xs text-fg-muted">Waiting to export</dt>
          <dd className="tabular-nums">
            {formatCount(sink.lag_rows)} rows
            {sink.lag_secs === null ? "" : `, oldest ${formatDuration(sink.lag_secs)} old`}
          </dd>
        </div>
        <div className="flex flex-col">
          <dt className="text-xs text-fg-muted">Last success</dt>
          <dd>
            {sink.last_success_at === null
              ? "Never"
              : formatInstant(sink.last_success_at, timeZone)}
          </dd>
        </div>
        {sink.state === "failing" ? (
          <div className="flex flex-col">
            <dt className="text-xs text-fg-muted">Failing since</dt>
            <dd>{formatInstant(sink.since, timeZone)}</dd>
          </div>
        ) : null}
      </dl>
      {sink.state === "failing" ? <p className="text-sm text-danger">{sink.last_error}</p> : null}
      {looksLikeProtectedFolder(sink) ? (
        <p role="note" className="text-sm">
          macOS may be blocking this folder (Documents, Desktop, Downloads, iCloud Drive or cloud
          storage). Point the sink at a folder outside them, or grant access in System Settings,
          Privacy and Security, Full Disk Access (or Files and Folders) by adding
          TokenMtr.app/Contents/MacOS/tokenmtr.
        </p>
      ) : null}
      {sink.recent_runs.length === 0 ? (
        <p className="text-sm text-fg-muted">No run recorded yet.</p>
      ) : (
        <DataTable
          caption={`Run history of ${sink.sink}`}
          columns={runColumns(timeZone)}
          rows={sink.recent_runs}
          rowKey={(run) => `${run.dataset}/${run.started_at}`}
        />
      )}
      {canExport(sink) ? null : (
        <p className="text-sm text-fg-muted">This sink is switched off in the config.</p>
      )}
    </li>
  );
}
