import { type ReactNode, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { useAppConfig } from "../../shared/app-context";
import { CostChart, seriesGroups } from "../../shared/charts";
import { formatPercent, formatRoiMultiple, formatTokens } from "../../shared/format";
import {
  NO_USAGE_COPY,
  queryViewState,
  useHeatmap,
  useModels,
  useOverview,
  useProjects,
  useQuotaLatest,
  useSeries,
} from "../../shared/queries";
import { QuotaMeters } from "../../shared/quota-meters";
import { RankedBars } from "../../shared/ranked-bars";
import type { GroupBy } from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
import { StateView } from "../../shared/state-view";
import {
  Amount,
  Card,
  ChartFrame,
  type Column,
  DataTable,
  Kpi,
  KpiGrid,
  PageHeader,
} from "../../shared/ui";
import { HeatmapGrid, summarizeWeekdays, type WeekdaySummary } from "./heatmap";

const TOP_COUNT = 5;

const COST_GROUPS: readonly { value: Extract<GroupBy, "agent" | "model">; label: string }[] = [
  { value: "agent", label: "By agent" },
  { value: "model", label: "By model" },
];

const WEEKDAY_COLUMNS = (currency: "USD" | "EUR"): Column<WeekdaySummary>[] => [
  { header: "Weekday", cell: (row) => row.weekday },
  {
    header: "Events",
    numeric: true,
    cell: (row) => row.events.toLocaleString("en-US"),
  },
  { header: "Tokens", numeric: true, cell: (row) => formatTokens(row.tokens) },
  {
    header: "API value",
    numeric: true,
    cell: (row) => <Amount micros={row.apiValueMicros} currency={currency} />,
  },
  {
    header: "Busiest hour",
    numeric: true,
    cell: (row) =>
      row.busiestHour === null ? "—" : `${String(row.busiestHour).padStart(2, "0")}:00`,
  },
];

function Kpis(): ReactNode {
  const { filters } = useFilters();
  const query = useOverview(filters);
  const totals = query.data;
  if (totals === undefined) {
    return null;
  }
  const { currency, approximate } = totals;
  return (
    <KpiGrid>
      <Kpi
        label="Paid"
        value={<Amount micros={totals.paid_micros} currency={currency} approximate={approximate} />}
        detail={totals.currency_fallback ? "EUR rates unavailable: shown in USD" : undefined}
      />
      <Kpi
        label="API value"
        value={
          <Amount micros={totals.api_value_micros} currency={currency} approximate={approximate} />
        }
        detail={approximate ? "Includes unpriced usage" : undefined}
      />
      <Kpi label="ROI" value={formatRoiMultiple(totals.roi)} detail="API value over fees" />
      <Kpi label="Tokens" value={formatTokens(totals.total_tokens)} />
      <Kpi
        label="Cached input"
        value={formatPercent(totals.cached_input_percent / 100)}
        detail="Share of input read from cache"
      />
      <Kpi
        label="Cache savings"
        value={
          totals.cache_savings_micros === null ? (
            "—"
          ) : (
            <Amount micros={totals.cache_savings_micros} currency={currency} />
          )
        }
      />
      <Kpi label="Output tokens" value={formatTokens(totals.output)} />
      <Kpi label="Sessions" value={totals.sessions.toLocaleString("en-US")} />
    </KpiGrid>
  );
}

function CostCard(): ReactNode {
  const { filters } = useFilters();
  const [groupBy, setGroupBy] = useState<"agent" | "model">("agent");
  const query = useSeries(filters, groupBy);
  const series = query.data;
  if (series === undefined) {
    return null;
  }
  const groups = seriesGroups(series);
  return (
    <ChartFrame
      title="Daily cost"
      description={`Cost per day, grouped by ${groupBy}: ${series.points.length} points.`}
      points={series.points.length}
      actions={
        <SegmentedControl
          label="Group cost"
          options={COST_GROUPS}
          value={groupBy}
          onChange={setGroupBy}
        />
      }
      table={
        <DataTable
          caption="Daily API value"
          rows={series.points}
          rowKey={(point) => point.start}
          columns={[
            {
              header: "Day",
              cell: (point) => point.start,
              sortValue: (point) => point.start,
            },
            ...groups.map(
              (group): Column<(typeof series.points)[number]> => ({
                header: group,
                numeric: true,
                cell: (point) => (
                  <Amount
                    micros={point.groups[group]?.api_value_micros ?? 0}
                    currency={series.currency}
                  />
                ),
                sortValue: (point) => point.groups[group]?.api_value_micros ?? 0,
              }),
            ),
            {
              header: "Paid",
              numeric: true,
              cell: (point) => <Amount micros={point.paid_micros} currency={series.currency} />,
              sortValue: (point) => point.paid_micros,
            },
          ]}
        />
      }
    >
      <div className="h-56">
        <CostChart series={series} currency={series.currency} />
      </div>
    </ChartFrame>
  );
}

function TopModels(): ReactNode {
  const { filters, setFilters } = useFilters();
  const query = useModels(filters);
  const rows = (query.data ?? []).slice(0, TOP_COUNT);
  return (
    <Card title="Top models">
      {rows.length === 0 ? (
        <p className="text-sm text-fg-muted">No model usage in this range.</p>
      ) : (
        <RankedBars
          label="Top models by API value"
          items={rows.map((row) => ({
            key: row.model,
            label: row.model,
            value: (
              <Amount
                micros={row.api_value_micros}
                currency={filters.currency}
                approximate={row.approximate}
              />
            ),
            percent: row.share_percent,
            detail: `${formatPercent(row.share_percent / 100)} of cost · ${formatTokens(row.total_tokens)} tokens`,
            onPress: () => setFilters({ ...filters, model: row.model }),
          }))}
        />
      )}
    </Card>
  );
}

function TopProjects(): ReactNode {
  const { filters, setFilters } = useFilters();
  const query = useProjects(filters);
  const rows = (query.data ?? []).slice(0, TOP_COUNT);
  return (
    <Card title="Top projects">
      {rows.length === 0 ? (
        <p className="text-sm text-fg-muted">No project usage in this range.</p>
      ) : (
        <RankedBars
          label="Top projects by API value"
          items={rows.map((row) => {
            const name = row.name ?? row.project ?? "No project";
            return {
              key: row.project ?? "(none)",
              label: name,
              value: (
                <Amount
                  micros={row.api_value_micros}
                  currency={filters.currency}
                  approximate={row.approximate}
                />
              ),
              percent: row.share_percent,
              detail: row.client ?? undefined,
              ...(row.project === null
                ? {}
                : { onPress: () => setFilters({ ...filters, project: row.project }) }),
            };
          })}
        />
      )}
    </Card>
  );
}

function Activity(): ReactNode {
  const { filters } = useFilters();
  const heatmap = useHeatmap(filters).data;
  if (heatmap === undefined) {
    return null;
  }
  return (
    <Card title="Activity by hour">
      <HeatmapGrid heatmap={heatmap} />
      <details className="text-sm">
        <summary className="cursor-pointer font-medium text-accent">View as table</summary>
        <div className="pt-2">
          <DataTable
            caption="Activity by weekday"
            rows={summarizeWeekdays(heatmap)}
            rowKey={(row) => row.weekday}
            columns={WEEKDAY_COLUMNS(heatmap.currency)}
          />
        </div>
      </details>
    </Card>
  );
}

function QuotaSection(): ReactNode {
  const { filters } = useFilters();
  const { timeZone, now } = useAppConfig();
  const meters = useQuotaLatest(filters).data ?? [];
  if (meters.length === 0) {
    return null;
  }
  return (
    <Card title="Quota">
      <QuotaMeters meters={meters} timeZone={timeZone} now={now()} />
    </Card>
  );
}

/** Overview (PLAN §7): quota, headline figures, daily cost, top models and projects, activity. */
export function OverviewScreen(): ReactNode {
  const { filters } = useFilters();
  const totals = useOverview(filters);
  const state = queryViewState(totals, (data) => (data.events === 0 ? NO_USAGE_COPY : null));
  return (
    <>
      <PageHeader
        title="Overview"
        summary="Paid cost against API value, quota meters, daily cost, top models and projects."
      />
      <StateView state={state}>
        <QuotaSection />
        <Kpis />
        <CostCard />
        <div className="grid min-w-0 gap-4 lg:grid-cols-2">
          <TopModels />
          <TopProjects />
        </div>
        <Activity />
      </StateView>
    </>
  );
}
