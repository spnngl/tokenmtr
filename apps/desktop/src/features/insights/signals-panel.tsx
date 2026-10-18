import type { ReactNode } from "react";
import { Bar, BarChart, CartesianGrid, Legend, Tooltip, XAxis, YAxis } from "recharts";
import { chartColor } from "../../shared/charts";
import { ResponsiveChart } from "../../shared/responsive-chart";
import type { SignalsReport } from "../../shared/schemas/usage";
import { ChartFrame, type Column, DataTable, Kpi, KpiGrid, Section } from "../../shared/ui";
import { useChartAnimation } from "../../shared/use-chart-animation";
import { type SignalDay, signalPoints } from "./model";

type SessionRow = SignalsReport["top_sessions"][number];

const DAY_COLUMNS: readonly Column<SignalDay>[] = [
  { header: "Day", cell: (row) => row.date },
  { header: "API errors", numeric: true, cell: (row) => row.api_errors },
  { header: "Compactions", numeric: true, cell: (row) => row.compactions },
];

const SESSION_COLUMNS: readonly Column<SessionRow>[] = [
  { header: "Agent", cell: (row) => row.agent },
  { header: "Session", cell: (row) => <code className="text-xs">{row.session}</code> },
  { header: "Compactions", numeric: true, cell: (row) => row.compactions },
];

const AXIS_STYLE = { fontSize: 12, fill: "var(--fg-muted)" } as const;

function DailyChart({ days }: { readonly days: readonly SignalDay[] }): ReactNode {
  const animate = useChartAnimation();
  return (
    <ResponsiveChart height={220}>
      {(size) => (
        <BarChart
          width={size.width}
          height={size.height}
          data={days}
          accessibilityLayer={false}
          margin={{ top: 4, right: 8, bottom: 4, left: 0 }}
        >
          <CartesianGrid vertical={false} stroke="var(--border)" />
          <XAxis dataKey="date" tick={AXIS_STYLE} minTickGap={32} stroke="var(--border)" />
          <YAxis allowDecimals={false} tick={AXIS_STYLE} width={32} stroke="var(--border)" />
          <Tooltip />
          <Legend />
          <Bar
            name="API errors"
            dataKey="api_errors"
            fill="var(--danger)"
            isAnimationActive={animate}
          />
          <Bar
            name="Compactions"
            dataKey="compactions"
            fill={chartColor(1)}
            isAnimationActive={animate}
          />
        </BarChart>
      )}
    </ResponsiveChart>
  );
}

/** API errors and compactions over time, and the sessions that compacted most. */
export function SignalsPanel({ report }: { readonly report: SignalsReport }): ReactNode {
  const days = signalPoints(report.days);
  return (
    <Section
      title="Errors and compactions"
      description="API errors and context compactions per day."
    >
      <KpiGrid>
        <Kpi label="API errors" value={report.api_errors.toLocaleString("en-US")} />
        <Kpi label="Compactions" value={report.compactions.toLocaleString("en-US")} />
      </KpiGrid>
      {days.length === 0 ? (
        <p className="text-sm text-fg-muted">No errors or compactions in this range.</p>
      ) : (
        <ChartFrame
          title="API errors and compactions per day"
          description={`${days.length} points from ${days[0]?.date ?? ""} to ${days.at(-1)?.date ?? ""}.`}
          points={days.length}
          table={
            <DataTable
              caption="Signals per day"
              columns={DAY_COLUMNS}
              rows={days}
              rowKey={(row) => row.date}
            />
          }
        >
          <DailyChart days={days} />
        </ChartFrame>
      )}
      {report.top_sessions.length === 0 ? null : (
        <DataTable
          caption="Compactions per session"
          columns={SESSION_COLUMNS}
          rows={report.top_sessions}
          rowKey={(row) => `${row.agent}/${row.session}`}
        />
      )}
    </Section>
  );
}
