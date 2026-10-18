import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useMemo, useState } from "react";
import { toRemote, useDaemonGate } from "../../features/common/use-remote";
import { localGenerateReport } from "../../features/reports/local-report";
import { ReportsScreen } from "../../features/reports/reports-screen";
import type { ReportRequest, ReportScope } from "../../features/reports/request";
import { downloadReport, printHtml } from "../../features/reports/save";
import { useDataSource } from "../../shared/app-context";
import type { Filters } from "../../shared/filters";
import { useFilterOptions } from "../../shared/queries";
import { useFilters } from "../use-filters";

function scopeOf(filters: Filters): ReportScope {
  if (filters.project !== null) {
    return { kind: "project", project: filters.project };
  }
  if (filters.client !== null) {
    return { kind: "client", client: filters.client };
  }
  return { kind: "all" };
}

/** Reports: period, scope and format pickers over a live preview. Starts from the filter bar. */
export function ReportsRoute(): ReactNode {
  const { filters } = useFilters();
  const dataSource = useDataSource();
  const options = useFilterOptions();
  const gate = useDaemonGate();
  const generate = useMemo(() => localGenerateReport(dataSource), [dataSource]);
  const [edited, setEdited] = useState<ReportRequest | null>(null);
  const request: ReportRequest = edited ?? {
    range: filters.range,
    scope: scopeOf(filters),
    format: "markdown",
    currency: filters.currency,
  };
  const preview = toRemote(
    useQuery({ queryKey: ["report", request], queryFn: () => generate(request) }),
    gate,
  );
  return (
    <ReportsScreen
      request={request}
      options={{
        projects: options.data?.projects ?? [],
        clients: options.data?.clients ?? [],
      }}
      onRequestChange={setEdited}
      preview={preview}
      onSave={downloadReport}
      onPrint={(printRequest) => {
        void generate({ ...printRequest, format: "html" }).then(printHtml);
      }}
    />
  );
}
