import { type ReactNode, useState } from "react";
import { useFilters } from "../../app/use-filters";
import { useAppConfig } from "../../shared/app-context";
import { CostChart, seriesGroups } from "../../shared/charts";
import {
  type Currency,
  formatCount,
  formatPercent,
  formatRoiMultiple,
  formatTokens,
  PLACEHOLDER,
} from "../../shared/format";
import {
  useHeatmap,
  useModels,
  useOverview,
  useProjects,
  useQuotaLatest,
  useSeries,
} from "../../shared/queries";
import { QuotaMeters } from "../../shared/quota-meters";
import { RankedBars } from "../../shared/ranked-bars";
import { CardRemote, NO_USAGE_COPY, RemoteView } from "../../shared/remote-view";
import type { GroupBy, Series, Totals } from "../../shared/schemas";
import { SegmentedControl } from "../../shared/segmented-control";
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
import { toRemote } from "../../shared/use-remote";
import { ActivityCalendarCard, type CalendarMetric } from "./activity-calendar";
import { HeatmapGrid, summarizeWeekdays, type WeekdaySummary } from "./heatmap";

const TOP_COUNT = 5;

const COST_GROUPS: readonly { value: Extract<GroupBy, "agent" | "model">; label: string }[] = [
  { value: "model", label: "By model" },
  { value: "agent", label: "By agent" },
];

const WEEKDAY_COLUMNS = (currency: Currency): Column<WeekdaySummary>[] => [
  { header: "Weekday", cell: (row) => row.weekday },
  { header: "Events", numeric: true, cell: (row) => formatCount(row.events) },
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
      row.busiestHour === null ? PLACEHOLDER : `${String(row.busiestHour).padStart(2, "0")}:00`,
  },
];

/** The FX fallback first; otherwise the Claude extra usage inside Paid, when any. */
function paidDetail(totals: Totals): ReactNode {
  if (totals.currency_fallback) {
    return "EUR rates unavailable: shown in USD";
  }
  if (totals.extra_usage_micros === 0) {
    return undefined;
  }
  return (
    <>
      Includes{" "}
      <Amount
        micros={totals.extra_usage_micros}
        currency={totals.currency}
        approximate={totals.approximate}
      />{" "}
      extra usage
    </>
  );
}

function Kpis({ totals }: { readonly totals: Totals }): ReactNode {
  const { currency, approximate } = totals;
  return (
    <KpiGrid>
      <Kpi
        label="Paid"
        value={<Amount micros={totals.paid_micros} currency={currency} approximate={approximate} />}
        detail={paidDetail(totals)}
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
            PLACEHOLDER
          ) : (
            <Amount micros={totals.cache_savings_micros} currency={currency} />
          )
        }
      />
      <Kpi label="Output tokens" value={formatTokens(totals.output)} />
      <Kpi label="Sessions" value={formatCount(totals.sessions)} />
    </KpiGrid>
  );
}

function CostCard(): ReactNode {
  const { filters } = useFilters();
  const [groupBy, setGroupBy] = useState<"agent" | "model">("model");
  return (
    <CardRemote title="Daily cost" remote={toRemote(useSeries(filters, groupBy))}>
      {(series) => <CostFrame series={series} groupBy={groupBy} onGroupByChange={setGroupBy} />}
    </CardRemote>
  );
}

function CostFrame(props: {
  readonly series: Series;
  readonly groupBy: "agent" | "model";
  readonly onGroupByChange: (groupBy: "agent" | "model") => void;
}): ReactNode {
  const { series, groupBy } = props;
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
          onChange={props.onGroupByChange}
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
  return (
    <Card title="Top models">
      <RemoteView remote={toRemote(useModels(filters))} size="section">
        {({ currency, rows: models }) =>
          models.length === 0 ? (
            <p className="text-sm text-fg-muted">No model usage in this range.</p>
          ) : (
            <RankedBars
              label="Top models by API value"
              items={models.slice(0, TOP_COUNT).map((row) => ({
                key: row.model,
                label: row.model,
                value: (
                  <Amount
                    micros={row.api_value_micros}
                    currency={currency}
                    approximate={row.approximate}
                  />
                ),
                percent: row.share_percent,
                detail: `${formatPercent(row.share_percent / 100)} of cost · ${formatTokens(row.total_tokens)} tokens`,
                onPress: () => setFilters({ ...filters, model: row.model }),
              }))}
            />
          )
        }
      </RemoteView>
    </Card>
  );
}

function TopProjects(): ReactNode {
  const { filters, setFilters } = useFilters();
  return (
    <Card title="Top projects">
      <RemoteView remote={toRemote(useProjects(filters))} size="section">
        {({ currency, rows: projects }) =>
          projects.length === 0 ? (
            <p className="text-sm text-fg-muted">No project usage in this range.</p>
          ) : (
            <RankedBars
              label="Top projects by API value"
              items={projects.slice(0, TOP_COUNT).map((row) => {
                const { project } = row;
                return {
                  key: project ?? "(none)",
                  label: row.name ?? project ?? "No project",
                  value: (
                    <Amount
                      micros={row.api_value_micros}
                      currency={currency}
                      approximate={row.approximate}
                    />
                  ),
                  percent: row.share_percent,
                  detail: row.client ?? undefined,
                  ...(project === null
                    ? {}
                    : { onPress: () => setFilters({ ...filters, project }) }),
                };
              })}
            />
          )
        }
      </RemoteView>
    </Card>
  );
}

function Activity(): ReactNode {
  const { filters } = useFilters();
  return (
    <Card title="Activity by hour">
      <RemoteView remote={toRemote(useHeatmap(filters))} size="section">
        {(heatmap) => (
          <>
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
          </>
        )}
      </RemoteView>
    </Card>
  );
}

/** Hidden when no followed agent reports quota; a failed read still shows, with Retry. */
function QuotaSection(): ReactNode {
  const { filters } = useFilters();
  const { timeZone, now } = useAppConfig();
  return (
    <CardRemote title="Quota" remote={toRemote(useQuotaLatest(filters))}>
      {(meters) =>
        meters.length === 0 ? null : (
          <Card title="Quota">
            <QuotaMeters meters={meters} timeZone={timeZone} now={now()} />
          </Card>
        )
      }
    </CardRemote>
  );
}

/** Overview: quota, headline figures, daily cost, top models and projects, activity. */
export function OverviewScreen(): ReactNode {
  const { filters } = useFilters();
  const overview = toRemote(useOverview(filters));
  // The calendar ignores the range: it still has a story to tell when the range is idle.
  const idleRange = overview.kind === "ready" && overview.data.events === 0;
  // Here, not in the card: the card moves when the range turns idle.
  const [calendarMetric, setCalendarMetric] = useState<CalendarMetric>("tokens");
  const calendar = (
    <ActivityCalendarCard metric={calendarMetric} onMetricChange={setCalendarMetric} />
  );
  return (
    <>
      <PageHeader to="/" />
      <RemoteView
        remote={overview}
        emptyWhen={(totals) => (totals.events === 0 ? NO_USAGE_COPY : null)}
      >
        {(totals) => (
          <>
            <QuotaSection />
            <Kpis totals={totals} />
            <CostCard />
            <div className="grid min-w-0 gap-4 lg:grid-cols-2">
              <TopModels />
              <TopProjects />
            </div>
            {calendar}
            <Activity />
          </>
        )}
      </RemoteView>
      {idleRange ? calendar : null}
    </>
  );
}
