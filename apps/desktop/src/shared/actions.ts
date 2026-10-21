import { type UseMutationResult, useMutation, useQueryClient } from "@tanstack/react-query";
import { useDataSource } from "./app-context";
import type { DataSource, RevealTarget } from "./data-source";
import { errorText } from "./errors";
import { useNotify } from "./toasts";

/** How an action reports its outcome in the toast region. */
export interface ActionCopy<Result, Variables> {
  /** Toast title when the action fails; the error's sentence becomes its body. */
  readonly failure: string;
  /** Toast after the action succeeded; omit when the screen itself shows the change. */
  readonly success?: (result: Result, variables: Variables) => string | null;
  /** Refetch every cached read on success (an action may change anything). Default `true`. */
  readonly refetch?: boolean;
}

/**
 * A `DataSource` action as a mutation: `isPending` for the control that triggers it, a toast on
 * failure (and on success when `copy.success` says so), and a refetch of every cached read on
 * success.
 *
 * `run` must be stable in meaning, not identity: it is called with the data source and the
 * variables passed to `mutate`.
 */
export function useDataAction<Variables = void, Result = unknown>(
  run: (dataSource: DataSource, variables: Variables) => Promise<Result>,
  copy: ActionCopy<Result, Variables>,
): UseMutationResult<Result, Error, Variables> {
  const dataSource = useDataSource();
  const queryClient = useQueryClient();
  const notify = useNotify();
  return useMutation({
    mutationFn: (variables: Variables) => run(dataSource, variables),
    onSuccess: (result, variables) => {
      const title = copy.success?.(result, variables) ?? null;
      if (title !== null) {
        notify({ tone: "success", title });
      }
      return copy.refetch === false ? undefined : queryClient.invalidateQueries();
    },
    onError: (error) => {
      notify({ tone: "error", title: copy.failure, message: errorText(error).message });
    },
  });
}

/** Scan every agent's logs now (daemon, or in-process when it is not running). */
export const useScanNow = () =>
  useDataAction((ds) => ds.scanNow(), { failure: "Scan failed", success: () => "Scan finished" });

/** Run every export sink now. */
export const useExportNow = () =>
  useDataAction((ds) => ds.exportNow(), {
    failure: "Export failed",
    success: () => "Export finished",
  });

/** Write a database backup now. */
export const useBackupNow = () =>
  useDataAction((ds) => ds.backupNow(), {
    failure: "Backup failed",
    success: () => "Backup saved",
  });

/** Show a known file or folder in the OS file manager; changes no data, so nothing refetches. */
export const useRevealPath = () =>
  useDataAction<RevealTarget>((ds, target) => ds.revealPath(target), {
    failure: "Could not open the file",
    refetch: false,
  });

/** Show the config file in the OS file manager, where price overrides are added. */
export const useRevealConfig = () =>
  useDataAction((ds) => ds.revealPath({ kind: "config_file" }), {
    failure: "Could not open the config file",
    refetch: false,
  });

/** Open the config file in the default text editor; changes no data, so nothing refetches. */
export const useEditConfig = () =>
  useDataAction((ds) => ds.editConfig(), {
    failure: "Could not open the config file",
    refetch: false,
  });

/** Quit the app; the daemon keeps collecting. */
export const useQuit = () =>
  useDataAction((ds) => ds.quit(), { failure: "Could not quit", refetch: false });

/** Install the daemon's service and start it (also starts an installed, stopped daemon). */
export const useInstallDaemon = () =>
  useDataAction((ds) => ds.installDaemon(), { failure: "Could not start the daemon" });

/** Restart the running daemon (after an update, or when it stopped answering). */
export const useRestartDaemon = () =>
  useDataAction((ds) => ds.restartDaemon(), { failure: "Could not restart the daemon" });

/** Keep one agent profile's transcripts for ten years; `label` names the agent in errors. */
export const useKeepTenYears = (agent: string, profile: string, label: string) =>
  useDataAction((ds) => ds.keepTenYears(agent, profile), {
    failure: `Could not change ${label} retention`,
  });

/**
 * Re-read the Claude credentials now, macOS asking when the Keychain refuses the daemon, then
 * poll quota again without waiting for a retry delay.
 */
export const useReloadClaudeCredentials = () =>
  useDataAction((ds) => ds.connectKeychain(), {
    failure: "Could not reload the Claude credentials",
    success: () => "Claude credentials reloaded",
  });
