import { type ReactNode, useState } from "react";
import { Button } from "../../shared/button";
import type { SystemHealth } from "../../shared/schemas";
import { Badge } from "../common/badge";
import { type Column, DataTable } from "../common/data-table";
import { PageHeader, Section } from "../common/page";
import { type Remote, RemoteView } from "../common/remote-view";
import { type DatasetStatus, datasetStatuses, formatInstant } from "./model";
import { SinkCard } from "./sink-card";

interface ExportScreenProps {
  readonly remote: Remote<SystemHealth>;
  /** Runs an export of every enabled sink now; rejects with an `Error` the screen shows. */
  readonly onExportNow: () => Promise<void>;
  readonly timeZone: string;
}

type Action =
  | { readonly kind: "idle" }
  | { readonly kind: "running" }
  | { readonly kind: "done" }
  | { readonly kind: "failed"; readonly message: string };

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
      cell: (dataset) => dataset.lastRun?.rows.toLocaleString("en-US") ?? "n/a",
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
  timeZone,
}: {
  readonly health: SystemHealth;
  readonly onExportNow: () => Promise<void>;
  readonly timeZone: string;
}): ReactNode {
  const [action, setAction] = useState<Action>({ kind: "idle" });
  const run = (): void => {
    setAction({ kind: "running" });
    onExportNow().then(
      () => setAction({ kind: "done" }),
      (error: unknown) =>
        setAction({
          kind: "failed",
          message: error instanceof Error ? error.message : "Export failed",
        }),
    );
  };
  const busy = action.kind === "running";
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
          <Button variant="primary" isDisabled={busy} onPress={run}>
            {busy ? "Exporting" : "Export now"}
          </Button>
        }
      >
        {action.kind === "done" ? (
          <p role="status" className="text-sm">
            Export finished.
          </p>
        ) : null}
        {action.kind === "failed" ? (
          <p role="alert" className="text-sm text-danger">
            {action.message}
          </p>
        ) : null}
        <ul className="flex flex-col gap-3">
          {health.sinks.map((sink) => (
            <SinkCard key={sink.sink} sink={sink} timeZone={timeZone} onRetry={run} busy={busy} />
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
      </Section>
    </div>
  );
}

/** The Export page: sink health (D41), run history, datasets and the "Export now" action. */
export function ExportScreen({ remote, onExportNow, timeZone }: ExportScreenProps): ReactNode {
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
        {(health) => <ExportBody health={health} onExportNow={onExportNow} timeZone={timeZone} />}
      </RemoteView>
    </div>
  );
}
