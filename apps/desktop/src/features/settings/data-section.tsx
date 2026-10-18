import { type ReactNode, useState } from "react";
import { Input, Label, TextField } from "react-aria-components";
import { useDataAction } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { Button } from "../../shared/button";
import { formatDateTime } from "../../shared/format";
import { useAlerts, useParseIssues } from "../../shared/queries";
import type { Alert, Backup, ParseIssue, Settings } from "../../shared/schemas";
import { Card, type Column, DataTable } from "../../shared/ui";

const ISSUE_LABELS: Record<ParseIssue["kind"], string> = {
  format_drift: "Format drift",
  unknown_version: "Unknown agent version",
  protected_folder: "Protected folder",
  missing_cwd: "Missing working directory",
  parse_error: "Parse error",
};

const CONFIRMATION = "erase all data";

function alertText(alert: Alert): string {
  switch (alert.kind) {
    case "quota_threshold":
      return `Quota window ${alert.window} reached ${alert.percent}%`;
    case "quota_reset":
      return `Quota window ${alert.window} reset`;
    case "budget_threshold":
      return `Budget ${alert.budget} reached ${alert.percent}%`;
    case "sink_failing":
      return `Sink ${alert.sink} is failing`;
    case "format_drift":
      return `${alert.agent} changed its log format`;
    case "backup_failed":
      return "Backup failed";
    default:
      alert satisfies never;
      throw new Error("unreachable alert kind");
  }
}

function ErrorLine({ error }: { readonly error: Error | null }): ReactNode {
  return error === null ? null : (
    <p role="alert" className="text-sm text-danger">
      {error.message}
    </p>
  );
}

/** D50: macOS asks before a background process reads Documents, Desktop, iCloud or Dropbox. */
export function ProtectedFoldersSection({
  folders,
}: {
  readonly folders: Settings["protected_folders"];
}): ReactNode {
  return (
    <Card title="Protected folders">
      {folders.length === 0 ? (
        <p className="text-sm text-fg-muted">
          No agent folder lives in a macOS-protected location.
        </p>
      ) : (
        <>
          <ul className="flex flex-col gap-1 text-sm">
            {folders.map((folder) => (
              <li key={`${folder.agent}:${folder.path}`}>
                <code>{folder.path}</code> ({folder.agent}):{" "}
                {folder.accessible ? "access granted" : "access not granted"}
              </li>
            ))}
          </ul>
          {folders.some((folder) => !folder.accessible) ? (
            <p role="status" className="text-sm text-fg-muted">
              TokenMtr cannot read these until you allow it: open System Settings, Privacy &amp;
              Security, Files and Folders (or Full Disk Access), and enable TokenMtr. Until then it
              falls back to the folders it can read.
            </p>
          ) : null}
        </>
      )}
    </Card>
  );
}

export function BackupsSection({ backups }: { readonly backups: readonly Backup[] }): ReactNode {
  const { timeZone } = useAppConfig();
  const backup = useDataAction((ds) => ds.backupNow());
  const columns: Column<Backup>[] = [
    {
      id: "at",
      header: "When",
      cell: (row) => formatDateTime(Date.parse(row.finished_at), timeZone),
    },
    {
      id: "result",
      header: "Result",
      cell: (row) => (row.error === null ? "OK" : `Failed: ${row.error}`),
    },
    { id: "path", header: "File", cell: (row) => row.path ?? "—" },
  ];
  return (
    <Card
      title="Backups"
      actions={
        <Button isPending={backup.isPending} onPress={() => backup.mutate()}>
          Back up now
        </Button>
      }
    >
      <ErrorLine error={backup.error} />
      {backups.length === 0 ? (
        <p className="text-sm text-fg-muted">No backup yet.</p>
      ) : (
        <DataTable
          caption="Backup history"
          rows={backups}
          rowKey={(row, index) => `${index}:${row.started_at}`}
          columns={columns}
        />
      )}
    </Card>
  );
}

export function ParseIssuesSection(): ReactNode {
  const { timeZone } = useAppConfig();
  const issues = useParseIssues().data ?? [];
  const open = issues.filter((issue) => issue.resolved_at === null);
  const drift = open.filter((issue) => issue.kind === "format_drift");
  const columns: Column<ParseIssue>[] = [
    { id: "kind", header: "Issue", cell: (row) => ISSUE_LABELS[row.kind] },
    { id: "agent", header: "Agent", cell: (row) => row.agent },
    { id: "subject", header: "Subject", cell: (row) => row.subject },
    {
      id: "count",
      header: "Count",
      align: "end",
      cell: (row) => row.occurrences.toLocaleString("en-US"),
    },
    {
      id: "last",
      header: "Last seen",
      cell: (row) => formatDateTime(Date.parse(row.last_seen), timeZone),
    },
    {
      id: "status",
      header: "Status",
      cell: (row) => (row.resolved_at === null ? "Open" : "Resolved"),
    },
  ];
  return (
    <Card title="Parse issues">
      {drift.length === 0 ? null : (
        <p
          role="status"
          className="rounded-md border border-border bg-surface-muted px-3 py-2 text-sm"
        >
          {drift.map((issue) => issue.agent).join(", ")} changed its log format. Usage from the new
          format may be missing until TokenMtr is updated.
        </p>
      )}
      {issues.length === 0 ? (
        <p className="text-sm text-fg-muted">No parse issue. Every log line was understood.</p>
      ) : (
        <DataTable
          caption="Parse issue list"
          rows={issues}
          rowKey={(row, index) => `${index}:${row.agent}:${row.kind}:${row.subject}`}
          columns={columns}
        />
      )}
    </Card>
  );
}

export function RecentAlertsSection(): ReactNode {
  const { timeZone } = useAppConfig();
  const alerts = useAlerts().data ?? [];
  return (
    <Card title="Recent alerts">
      {alerts.length === 0 ? (
        <p className="text-sm text-fg-muted">No alert has fired.</p>
      ) : (
        <ul className="flex flex-col gap-1 text-sm">
          {alerts.map((alert) => (
            <li key={alert.dedup_key}>
              {formatDateTime(Date.parse(alert.fired_at), timeZone)}: {alertText(alert)}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

/** D56: erasing is irreversible, so the button stays disabled until the phrase is typed. */
export function EraseSection(): ReactNode {
  const [typed, setTyped] = useState("");
  const erase = useDataAction((ds) => ds.eraseAllData());
  return (
    <Card title="Erase all data">
      <p className="text-sm text-fg-muted">
        Deletes the TokenMtr database, exports and backups on this machine. Agent logs are not
        touched. This cannot be undone.
      </p>
      <TextField value={typed} onChange={setTyped} className="flex max-w-sm flex-col gap-1">
        <Label className="text-sm font-medium">Type "{CONFIRMATION}" to confirm</Label>
        <Input className="min-h-9 rounded-md border border-border bg-surface px-2 text-sm" />
      </TextField>
      <div>
        <Button
          variant="primary"
          isDisabled={typed !== CONFIRMATION}
          isPending={erase.isPending}
          onPress={() => erase.mutate()}
        >
          Erase all data
        </Button>
      </div>
      <ErrorLine error={erase.error} />
    </Card>
  );
}
