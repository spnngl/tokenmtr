import type { ReactNode } from "react";
import { Button } from "../../shared/button";
import { formatCount, formatPercent, formatTokens } from "../../shared/format";
import type { ModelRow } from "../../shared/schemas/usage";
import { Amount, Badge, type Column, DataTable, Section } from "../../shared/ui";
import { costSourceMix, divergentModels, type ModelsData, unpricedModels } from "./model";

interface ModelsViewProps {
  readonly report: ModelsData;
  /** Opens the pricing override editor in Settings. */
  readonly onAddOverride: () => void;
}

function describeMix(row: ModelRow): string {
  const mix = costSourceMix(row);
  const parts: string[] = [];
  if (mix.reported > 0) {
    parts.push(`${formatPercent(mix.reported)} reported`);
  }
  if (mix.estimated > 0) {
    parts.push(`${formatPercent(mix.estimated)} estimated`);
  }
  if (mix.unpriced > 0) {
    parts.push(`${formatPercent(mix.unpriced)} unpriced`);
  }
  return parts.length === 0 ? "No events" : parts.join(", ");
}

function signedPercent(percent: number): string {
  const text = formatPercent(Math.abs(percent) / 100, 1);
  return percent > 0 ? `+${text}` : percent < 0 ? `-${text}` : text;
}

function columns(report: ModelsData): readonly Column<ModelRow>[] {
  return [
    {
      header: "Model",
      cell: (row) => (
        <span className="flex flex-wrap items-center gap-2">
          <span className="font-medium">{row.model}</span>
          {row.unpriced_events > 0 ? <Badge tone="warning">Unpriced</Badge> : null}
          {row.divergent ? <Badge tone="warning">Diverges</Badge> : null}
        </span>
      ),
    },
    { header: "Events", numeric: true, cell: (row) => formatCount(row.events) },
    { header: "Tokens", numeric: true, cell: (row) => formatTokens(row.total_tokens) },
    {
      header: "API value",
      numeric: true,
      cell: (row) => (
        <Amount
          micros={row.api_value_micros}
          currency={report.currency}
          approximate={row.approximate}
        />
      ),
    },
    {
      header: "Share",
      numeric: true,
      cell: (row) => (
        <span className="inline-flex items-center justify-end gap-2">
          <span aria-hidden className="h-2 w-16 rounded-md bg-surface-muted">
            <span
              className="block h-full rounded-md bg-accent"
              style={{ width: `${Math.min(100, Math.max(0, row.share_percent))}%` }}
            />
          </span>
          {formatPercent(row.share_percent / 100, 1)}
        </span>
      ),
    },
    { header: "Cost source", cell: describeMix },
    {
      header: "Reported vs estimated",
      numeric: true,
      cell: (row) =>
        row.reported_vs_estimated_percent === null
          ? "n/a"
          : signedPercent(row.reported_vs_estimated_percent),
    },
  ];
}

/** Per-model tokens, cost, share and cost-source quality, with the warnings above the table. */
export function ModelsView({ report, onAddOverride }: ModelsViewProps): ReactNode {
  const unpriced = unpricedModels(report.rows);
  const divergent = divergentModels(report.rows);
  return (
    <div className="flex flex-col gap-6">
      {unpriced.length === 0 ? null : (
        <div role="note" className="flex flex-col gap-2 rounded-md border border-border p-3">
          <Badge tone="warning">Unpriced models</Badge>
          <p className="text-sm">
            No price covers {unpriced.map((row) => row.model).join(", ")}. Their API value is a
            lower bound until you add a price override.
          </p>
          <div>
            <Button variant="secondary" onPress={onAddOverride}>
              Add price override
            </Button>
          </div>
        </div>
      )}
      {divergent.length === 0 ? null : (
        <div role="note" className="flex flex-col gap-2 rounded-md border border-border p-3">
          <Badge tone="warning">Reported and estimated costs differ</Badge>
          <p className="text-sm">
            {divergent.map((row) => row.model).join(", ")}: the cost the agent reported differs from
            our estimate by more than 5%.
          </p>
        </div>
      )}
      <Section
        title="Models"
        description="Biggest API value first. A leading ≈ marks an estimated or unpriced figure."
      >
        <DataTable
          caption="Usage per model"
          columns={columns(report)}
          rows={report.rows}
          rowKey={(row) => row.model}
        />
      </Section>
    </div>
  );
}
