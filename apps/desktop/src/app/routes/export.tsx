import { useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { ExportScreen } from "../../features/export/export-screen";
import { useAppConfig, useDataSource } from "../../shared/app-context";
import { useSystemHealth } from "../../shared/queries";
import { toRemote, useDaemonGate } from "../../shared/use-remote";

/** Export: sink health, run history, datasets and "Export now". */
export function ExportRoute(): ReactNode {
  const dataSource = useDataSource();
  const queryClient = useQueryClient();
  const { timeZone } = useAppConfig();
  const gate = useDaemonGate();
  const remote = toRemote(useSystemHealth(), gate);
  return (
    <ExportScreen
      remote={remote}
      timeZone={timeZone}
      onExportNow={async () => {
        try {
          await dataSource.exportNow();
        } finally {
          await queryClient.invalidateQueries({ queryKey: ["system-health"] });
        }
      }}
    />
  );
}
