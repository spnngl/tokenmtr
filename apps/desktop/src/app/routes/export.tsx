import type { ReactNode } from "react";
import { ExportScreen } from "../../features/export/export-screen";
import { useExportNow } from "../../shared/actions";
import { useAppConfig } from "../../shared/app-context";
import { useSystemHealth } from "../../shared/queries";
import { toRemote } from "../../shared/use-remote";

/** Export: sink health, run history, datasets and "Export now". */
export function ExportRoute(): ReactNode {
  const { timeZone } = useAppConfig();
  const exportNow = useExportNow();
  return (
    <ExportScreen
      remote={toRemote(useSystemHealth())}
      timeZone={timeZone}
      onExportNow={() => exportNow.mutate()}
      isExporting={exportNow.isPending}
    />
  );
}
