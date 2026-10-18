import { Download, Printer } from "lucide-react";
import type { ReactNode } from "react";
import { FilterSelect } from "../../app/filter-select";
import { Button } from "../../shared/button";
import { RANGE_PRESETS, type RangePreset } from "../../shared/filters";
import { type Remote, RemoteView } from "../../shared/remote-view";
import { type GeneratedReport, REPORT_FORMATS, type ReportScope } from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
import { PageHeader, Section } from "../../shared/ui";
import { formatLabel, type ReportRequest, rangeLabel } from "./request";

/** Where saving the report stands; `saved` carries the path the shell wrote. */
export type SaveStatus =
  | { readonly kind: "idle" }
  | { readonly kind: "saving" }
  | { readonly kind: "saved"; readonly path: string }
  | { readonly kind: "error"; readonly message: string };

/** Values the scope picker offers. */
export interface ReportOptions {
  readonly projects: readonly string[];
  readonly clients: readonly string[];
}

interface ReportsScreenProps {
  readonly request: ReportRequest;
  readonly options: ReportOptions;
  readonly onRequestChange: (request: ReportRequest) => void;
  readonly preview: Remote<GeneratedReport>;
  /** Renders the request and asks the shell where to save it. */
  readonly onSave: () => void;
  readonly saveStatus: SaveStatus;
  /** Renders the request as HTML and opens the print dialog ("Save as PDF"). */
  readonly onPrint: () => void;
  /** The HTML for printing is being rendered. */
  readonly isPrinting: boolean;
}

type ScopeKind = ReportScope["kind"];
type PeriodValue = RangePreset | "custom";

const PERIOD_LABELS: Record<RangePreset, string> = {
  today: "Today",
  "7d": "Last 7 days",
  "30d": "Last 30 days",
  "90d": "Last 90 days",
};

function scopeKinds(options: ReportOptions): readonly { value: ScopeKind; label: string }[] {
  return [
    { value: "all", label: "Everything" },
    ...(options.projects.length > 0 ? [{ value: "project" as const, label: "One project" }] : []),
    ...(options.clients.length > 0 ? [{ value: "client" as const, label: "One client" }] : []),
  ];
}

function scopeValue(scope: ReportScope): string {
  switch (scope.kind) {
    case "all":
      return "";
    case "project":
      return scope.project;
    case "client":
      return scope.client;
    default:
      return scope satisfies never;
  }
}

function Pickers({
  request,
  options,
  onRequestChange,
}: Pick<ReportsScreenProps, "request" | "options" | "onRequestChange">): ReactNode {
  const periods: { value: PeriodValue; label: string }[] = RANGE_PRESETS.map((preset) => ({
    value: preset,
    label: PERIOD_LABELS[preset],
  }));
  if (request.range.kind === "custom") {
    periods.push({ value: "custom", label: rangeLabel(request.range) });
  }
  const names = request.scope.kind === "client" ? options.clients : options.projects;
  return (
    <div className="flex flex-wrap items-end gap-3">
      <FilterSelect
        label="Period"
        options={periods}
        value={request.range.kind === "preset" ? request.range.preset : "custom"}
        onChange={(value) => {
          if (value !== "custom") {
            onRequestChange({ ...request, range: { kind: "preset", preset: value } });
          }
        }}
      />
      <FilterSelect
        label="Scope"
        options={scopeKinds(options)}
        value={request.scope.kind}
        onChange={(kind) => {
          switch (kind) {
            case "all":
              onRequestChange({ ...request, scope: { kind: "all" } });
              return;
            case "project": {
              const project = options.projects[0];
              if (project !== undefined) {
                onRequestChange({ ...request, scope: { kind: "project", project } });
              }
              return;
            }
            case "client": {
              const client = options.clients[0];
              if (client !== undefined) {
                onRequestChange({ ...request, scope: { kind: "client", client } });
              }
              return;
            }
            default:
              return kind satisfies never;
          }
        }}
      />
      {request.scope.kind === "all" ? null : (
        <FilterSelect
          label={request.scope.kind === "project" ? "Project" : "Client"}
          options={names.map((name) => ({ value: name, label: name }))}
          value={scopeValue(request.scope)}
          onChange={(name) =>
            onRequestChange({
              ...request,
              scope:
                request.scope.kind === "project"
                  ? { kind: "project", project: name }
                  : { kind: "client", client: name },
            })
          }
        />
      )}
      <SegmentedControl
        label="Format"
        options={REPORT_FORMATS.map((format) => ({ value: format, label: formatLabel(format) }))}
        value={request.format}
        onChange={(format) => onRequestChange({ ...request, format })}
      />
    </div>
  );
}

function Preview({ report }: { readonly report: GeneratedReport }): ReactNode {
  switch (report.format) {
    case "html":
      return (
        <iframe
          title="Report preview"
          sandbox=""
          srcDoc={report.content}
          className="h-96 w-full rounded-md border border-border bg-surface"
        />
      );
    case "markdown":
    case "csv":
      return (
        <section
          aria-label="Report preview"
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll region must be focusable (WCAG 2.1.1).
          tabIndex={0}
          className="max-h-96 overflow-auto rounded-md border border-border bg-surface-muted p-3"
        >
          <pre className="text-xs">{report.content}</pre>
        </section>
      );
    default:
      return report.format satisfies never;
  }
}

function SaveMessage({ status }: { readonly status: SaveStatus }): ReactNode {
  switch (status.kind) {
    case "idle":
    case "saving":
      return null;
    case "saved":
      return (
        <p role="status" className="text-sm text-fg-muted">
          Saved to {status.path}
        </p>
      );
    case "error":
      return (
        <p role="alert" className="text-sm text-danger">
          Could not save the report: {status.message}
        </p>
      );
    default:
      return status satisfies never;
  }
}

/** The Reports page: pick period, scope and format, preview live, save or print to PDF. */
export function ReportsScreen({
  request,
  options,
  onRequestChange,
  preview,
  onSave,
  saveStatus,
  onPrint,
  isPrinting,
}: ReportsScreenProps): ReactNode {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader to="/reports" />
      <Pickers request={request} options={options} onRequestChange={onRequestChange} />
      <RemoteView remote={preview} emptyWhen={() => null}>
        {(report) => (
          <Section
            title="Preview"
            description={report.filename}
            actions={
              <div className="flex flex-wrap gap-2">
                <Button variant="primary" isPending={saveStatus.kind === "saving"} onPress={onSave}>
                  {saveStatus.kind === "saving" ? null : (
                    <Download aria-hidden className="size-4" />
                  )}
                  Save {formatLabel(report.format)}
                </Button>
                <Button variant="secondary" isPending={isPrinting} onPress={onPrint}>
                  {isPrinting ? null : <Printer aria-hidden className="size-4" />}
                  Save as PDF
                </Button>
              </div>
            }
          >
            <SaveMessage status={saveStatus} />
            <Preview report={report} />
          </Section>
        )}
      </RemoteView>
    </div>
  );
}
