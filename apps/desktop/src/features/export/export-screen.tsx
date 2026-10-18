import type { ReactNode } from "react";
import { Button } from "../../shared/button";
import { formatCount } from "../../shared/format";
import { type Remote, RemoteView } from "../../shared/remote-view";
import type { SystemHealth } from "../../shared/schemas";
import { Badge, type Column, DataTable, PageHeader, Section } from "../../shared/ui";
import { type DatasetStatus, datasetStatuses, formatInstant } from "./model";
import { SinkCard } from "./sink-card";

interface ExportScreenProps {
  readonly remote: Remote<SystemHealth>;
  /** Runs an export of every enabled sink now; the app's toasts report the outcome. */
  readonly onExportNow: () => void;
  /** An export started by `onExportNow` is running. */
  readonly isExporting: boolean;
  readonly timeZone: string;
}

function datasetColumns(timeZone: string): readonly Column<DatasetStatus>[] {
  return [
    { header: "Dataset", cell: (dataset) => <code className="text-xs">{dataset.id}</code> },
    { header: "Content", cell: (dataset) => dataset.description },
    {
      header: "Last export",
      cell: (dataset) =>
        dataset.lastRun === null ? "Never" : formatInstant(dataset.lastRun.finished_at, timeZone),
    },
    {
      header: "Rows",
      numeric: true,
      cell: (dataset) => (dataset.lastRun === null ? "n/a" : formatCount(dataset.lastRun.rows)),
    },
    {
      header: "Result",
      cell: (dataset) =>
        dataset.lastRun === null ? (
          "n/a"
        ) : dataset.lastRun.error === null ? (
          <Badge tone="success">OK</Badge>
        ) : (
          <Badge tone="danger">Failed</Badge>
        ),
    },
  ];
}

function ExportBody({
  health,
  onExportNow,
  isExporting,
  timeZone,
}: {
  readonly health: SystemHealth;
  readonly onExportNow: () => void;
  readonly isExporting: boolean;
  readonly timeZone: string;
}): ReactNode {
  return (
    <div className="flex flex-col gap-8">
      {health.daemon.state === "running" ? null : (
        <p role="note" className="rounded-md border border-border p-3 text-sm">
          <Badge tone="warning">Daemon not running</Badge> Sinks only export while the daemon is
          running; start it from Settings.
        </p>
      )}
      <Section
        title="Sinks"
        description="Where your usage data is mirrored, and whether each copy is up to date."
        actions={
          <Button variant="primary" isPending={isExporting} onPress={onExportNow}>
            Export now
          </Button>
        }
      >
        <ul className="flex flex-col gap-3">
          {health.sinks.map((sink) => (
            <SinkCard
              key={sink.sink}
              sink={sink}
              timeZone={timeZone}
              onRetry={onExportNow}
              busy={isExporting}
            />
          ))}
        </ul>
      </Section>
      <Section title="Datasets" description="What a sink exports, and when each dataset last ran.">
        <DataTable
          caption="Exported datasets"
          columns={datasetColumns(timeZone)}
          rows={datasetStatuses(health.sinks)}
          rowKey={(dataset) => dataset.id}
        />
        {/* Exported rows are never deleted, only flagged. */}
        <p className="text-sm text-fg-muted">
          Exported rows are never deleted. A day of usage replaced by finer data stays with{" "}
          <code className="text-xs">superseded</code> set; a subscription removed from the config
          stays with <code className="text-xs">removed_at_utc</code> set.
        </p>
      </Section>
    </div>
  );
}

/** The Export page: sink health, run history, datasets and the "Export now" action. */
export function ExportScreen({
  remote,
  onExportNow,
  isExporting,
  timeZone,
}: ExportScreenProps): ReactNode {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader to="/export" />
      <RemoteView
        remote={remote}
        emptyWhen={(health) =>
          health.sinks.length === 0
            ? {
                title: "No export sink configured",
                hint: "Add a sink in the config to mirror your usage data to a file or a database.",
              }
            : null
        }
      >
        {(health) => (
          <ExportBody
            health={health}
            onExportNow={onExportNow}
            isExporting={isExporting}
            timeZone={timeZone}
          />
        )}
      </RemoteView>
    </div>
  );
}
