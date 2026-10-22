import type { ReactNode } from "react";
import { ExportScreen } from "../../features/export/export-screen";
import { useExportNow, useImportNow } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { useSystemHealth } from "../../shared/queries";
import { toRemote } from "../../shared/use-remote";

/** Export: sink and source health, run history, datasets, "Export now" and "Import now". */
export function ExportRoute(): ReactNode {
  const { timeZone } = useAppConfig();
  const exportNow = useExportNow();
  const importNow = useImportNow();
  return (
    <ExportScreen
      remote={toRemote(useSystemHealth())}
      timeZone={timeZone}
      onExportNow={() => exportNow.mutate()}
      isExporting={exportNow.isPending}
      onImportNow={() => importNow.mutate()}
      isImporting={importNow.isPending}
    />
  );
}
