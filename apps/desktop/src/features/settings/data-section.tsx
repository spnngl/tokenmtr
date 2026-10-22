import { type ReactNode, useState } from "react";
import { Input, Label, TextField } from "react-aria-components";
import { useBackupNow, useDataAction } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { APP_NAME } from "../../shared/brand";
import { Button } from "../../shared/button";
import { formatCount, formatDateTime, PLACEHOLDER } from "../../shared/format";
import { useAlerts, useParseIssues } from "../../shared/queries";
import { CardRemote } from "../../shared/remote-view";
import {
  type Alert,
  type Backup,
  INFORMATIONAL_ISSUE_KINDS,
  type ParseIssue,
  type Settings,
} from "../../shared/schemas";
import { Card, type Column, DataTable } from "../../shared/ui";
import { toRemote } from "../../shared/use-remote";

const ISSUE_LABELS: Record<ParseIssue["kind"], string> = {
  format_drift: "Format drift",
  unknown_version: "Unknown agent version",
  protected_folder: "Protected folder",
  missing_cwd: "Missing working directory",
  parse_error: "Parse error",
  integration_drift: "Superset data unreadable",
};

/** "Open" for a failure; an informational issue leaves the totals complete. */
function issueStatus(issue: ParseIssue): string {
  if (issue.resolved_at !== null) {
    return "Resolved";
  }
  return INFORMATIONAL_ISSUE_KINDS.has(issue.kind) ? "Info" : "Open";
}

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
    case "metrics_failing":
      return `Metrics output ${alert.output} is failing`;
    case "source_failing":
      return `Source ${alert.source} is failing`;
    case "format_drift":
      return `${alert.agent} changed its log format`;
    case "backup_failed":
      return "Backup failed";
    default:
      alert satisfies never;
      throw new Error("unreachable alert kind");
  }
}

/** macOS asks before a background process reads Documents, Desktop, iCloud or Dropbox. */
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
              {APP_NAME} cannot read these until you allow it: open System Settings, Privacy &amp;
              Security, Files and Folders (or Full Disk Access), and enable {APP_NAME}. Until then
              it falls back to the folders it can read.
            </p>
          ) : null}
        </>
      )}
    </Card>
  );
}

export function BackupsSection({ backups }: { readonly backups: readonly Backup[] }): ReactNode {
  const { timeZone } = useAppConfig();
  const backup = useBackupNow();
  const columns: Column<Backup>[] = [
    {
      header: "When",
      cell: (row) => formatDateTime(Date.parse(row.finished_at), timeZone),
    },
    {
      header: "Result",
      cell: (row) => (row.error === null ? "OK" : `Failed: ${row.error}`),
    },
    { header: "File", cell: (row) => row.path ?? PLACEHOLDER },
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

/** Parse issues; a failed read shows as an error, never as "no issue". */
export function ParseIssuesSection(): ReactNode {
  const issues = toRemote(useParseIssues());
  return (
    <CardRemote title="Parse issues" remote={issues}>
      {(issues) => <ParseIssuesCard issues={issues} />}
    </CardRemote>
  );
}

function ParseIssuesCard({ issues }: { readonly issues: readonly ParseIssue[] }): ReactNode {
  const { timeZone } = useAppConfig();
  const open = issues.filter((issue) => issue.resolved_at === null);
  const drift = open.filter((issue) => issue.kind === "format_drift");
  const columns: Column<ParseIssue>[] = [
    { header: "Issue", cell: (row) => ISSUE_LABELS[row.kind] },
    { header: "Agent", cell: (row) => row.agent },
    { header: "Subject", cell: (row) => row.subject },
    {
      header: "Count",
      numeric: true,
      cell: (row) => formatCount(row.occurrences),
    },
    {
      header: "Last seen",
      cell: (row) => formatDateTime(Date.parse(row.last_seen), timeZone),
    },
    {
      header: "Status",
      cell: issueStatus,
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
          format may be missing until {APP_NAME} is updated.
        </p>
      )}
      {open.some((issue) => INFORMATIONAL_ISSUE_KINDS.has(issue.kind)) ? (
        <p className="text-sm text-fg-muted">
          Info rows leave the totals complete. A missing working directory (a deleted worktree)
          keeps its usage under the directory until a project rule names it or Superset still knows
          the worktree.
        </p>
      ) : null}
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

/** Recently fired alerts; a failed read shows as an error, never as "no alert". */
export function RecentAlertsSection(): ReactNode {
  const alerts = toRemote(useAlerts());
  return (
    <CardRemote title="Recent alerts" remote={alerts}>
      {(alerts) => <RecentAlertsCard alerts={alerts} />}
    </CardRemote>
  );
}

function RecentAlertsCard({ alerts }: { readonly alerts: readonly Alert[] }): ReactNode {
  const { timeZone } = useAppConfig();
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

/** Erasing is irreversible, so the button stays disabled until the phrase is typed. */
export function EraseSection(): ReactNode {
  const [typed, setTyped] = useState("");
  const erase = useDataAction((ds) => ds.eraseAllData(), {
    failure: "Could not erase the data",
    success: () => "All data erased",
  });
  return (
    <Card title="Erase all data">
      <p className="text-sm text-fg-muted">
        Deletes the {APP_NAME} database, exports and backups on this machine. Agent logs are not
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
          onPress={() => erase.mutate(undefined, { onSuccess: () => setTyped("") })}
        >
          Erase all data
        </Button>
      </div>
    </Card>
  );
}
