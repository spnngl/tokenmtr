import { useMutation, useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { ReportsScreen, type SaveStatus } from "../../features/reports/reports-screen";
import { type ReportRequest, toReportSpec } from "../../features/reports/request";
import { printHtml } from "../../features/reports/save";
import { useDataAction } from "../../shared/actions";
import { useAppConfig, useDataSource } from "../../shared/app-context";
import { errorText } from "../../shared/errors";
import type { Filters } from "../../shared/filters";
import { formatDay } from "../../shared/format";
import { useFilterOptions } from "../../shared/queries";
import type { ReportScope } from "../../shared/schemas";
import { toRemote } from "../../shared/use-remote";
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
  const { timeZone, now } = useAppConfig();
  const [edited, setEdited] = useState<ReportRequest | null>(null);
  const request: ReportRequest = edited ?? {
    range: filters.range,
    scope: scopeOf(filters),
    format: "markdown",
    currency: filters.currency,
  };
  const spec = toReportSpec(request, formatDay(now(), timeZone));
  const preview = toRemote(
    useQuery({ queryKey: ["report", spec], queryFn: () => dataSource.generateReport(spec) }),
  );
  // Saving reports its outcome inline (the saved path stays visible); printing through a toast.
  const save = useMutation({ mutationFn: () => dataSource.saveReport(spec) });
  const print = useDataAction(
    async (ds) => printHtml(await ds.generateReport({ ...spec, format: "html" })),
    { failure: "Could not prepare the report for printing", refetch: false },
  );
  const saveStatus: SaveStatus = save.isPending
    ? { kind: "saving" }
    : save.isError
      ? { kind: "error", message: errorText(save.error).message }
      : typeof save.data === "string"
        ? { kind: "saved", path: save.data }
        : { kind: "idle" };
  return (
    <ReportsScreen
      request={request}
      options={{
        projects: options.data?.projects ?? [],
        clients: options.data?.clients ?? [],
      }}
      onRequestChange={(next) => {
        save.reset();
        setEdited(next);
      }}
      preview={preview}
      onSave={() => save.mutate()}
      saveStatus={saveStatus}
      onPrint={() => print.mutate()}
      isPrinting={print.isPending}
    />
  );
}
