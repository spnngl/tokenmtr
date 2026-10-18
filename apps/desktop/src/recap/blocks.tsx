import type { ReactNode } from "react";
import { formatClock, formatCurrencyMicros, formatPercent, formatTokens } from "../shared/format";
import { QuotaMeters } from "../shared/quota-meters";
import type { AgentRecap, Budget } from "../shared/schemas";
import { Amount } from "../shared/ui";
import { UsageMeter } from "../shared/usage-meter";

/** What every block needs to format its figures. */
export interface RecapContext {
  readonly nowMs: number;
  readonly timeZone: string;
}

const PLACEHOLDER = "\u2014";

function Block(props: { readonly title: string; readonly children: ReactNode }): ReactNode {
  return (
    <section aria-label={props.title} className="flex flex-col gap-2">
      {props.children}
    </section>
  );
}

function Stat(props: {
  readonly label: string;
  readonly children: ReactNode;
  readonly lead?: boolean;
}): ReactNode {
  return (
    <div className="flex min-w-0 flex-col">
      <dt className="text-xs text-fg-muted">{props.label}</dt>
      <dd className={props.lead ? "text-xl font-semibold" : "text-sm font-medium"}>
        {props.children}
      </dd>
    </div>
  );
}

function MoneyBlock({ recap }: { readonly recap: AgentRecap }): ReactNode {
  const { totals, currency } = recap;
  const approximate = totals.approximate;
  return (
    <Block title="Money">
      <dl className="grid grid-cols-2 gap-x-3 gap-y-2">
        <Stat label="Paid" lead>
          <Amount micros={totals.paid_micros} currency={currency} approximate={approximate} />
        </Stat>
        <Stat label="API value" lead>
          <Amount micros={totals.api_value_micros} currency={currency} approximate={approximate} />
        </Stat>
        {totals.roi === null ? null : <Stat label="ROI">{`${totals.roi.toFixed(1)}x`}</Stat>}
        <Stat label="Projected spend">
          <Amount micros={recap.projected_paid_micros} currency={currency} approximate />
        </Stat>
      </dl>
      {recap.currency_fallback ? (
        <p className="text-xs text-fg-muted">Shown in USD: no EUR rates yet.</p>
      ) : null}
    </Block>
  );
}

function budgetDetail(budget: Budget): ReactNode {
  const spent = formatCurrencyMicros(budget.spent_micros, budget.currency);
  const limit = formatCurrencyMicros(budget.limit_micros, budget.currency);
  const projected = formatCurrencyMicros(budget.projected_micros, budget.currency);
  return `${spent} of ${limit} · projected ${projected}`;
}

function BudgetBlock({ budgets }: { readonly budgets: readonly Budget[] }): ReactNode {
  if (budgets.length === 0) {
    return null;
  }
  return (
    <Block title="Budgets">
      {budgets.map((budget) => (
        <UsageMeter
          key={budget.id}
          label={budget.id}
          usedFraction={budget.percent / 100}
          detail={budgetDetail(budget)}
        />
      ))}
    </Block>
  );
}

function TokensBlock({ recap }: { readonly recap: AgentRecap }): ReactNode {
  const { totals } = recap;
  return (
    <Block title="Tokens">
      <dl className="grid grid-cols-3 gap-x-3">
        <Stat label="Tokens">{formatTokens(totals.total_tokens)}</Stat>
        <Stat label="Cached input">{formatPercent(totals.cached_input_percent / 100)}</Stat>
        <Stat label="Output">{formatTokens(totals.output)}</Stat>
      </dl>
    </Block>
  );
}

const SPARK_WIDTH = 100;
const SPARK_HEIGHT = 24;

function sparklinePoints(values: readonly number[]): string {
  const max = Math.max(...values, 1);
  const step = values.length > 1 ? SPARK_WIDTH / (values.length - 1) : 0;
  return values
    .map(
      (value, index) =>
        `${(index * step).toFixed(1)},${(SPARK_HEIGHT - (value / max) * SPARK_HEIGHT).toFixed(1)}`,
    )
    .join(" ");
}

function TrendBlock({ recap }: { readonly recap: AgentRecap }): ReactNode {
  const values = recap.sparkline.map((point) => point.api_value_micros);
  return (
    <Block title="Trend">
      <div className="flex items-center gap-3">
        <svg
          viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}
          preserveAspectRatio="none"
          role="img"
          aria-label="API value, last 7 days"
          className="h-6 flex-1 overflow-visible"
        >
          <polyline
            points={sparklinePoints(values)}
            fill="none"
            strokeWidth={2}
            vectorEffect="non-scaling-stroke"
            className="stroke-chart-1"
          />
        </svg>
        <span className="text-xs text-fg-muted">
          7-day peak <Amount micros={Math.max(...values, 0)} currency={recap.currency} />
        </span>
      </div>
    </Block>
  );
}

function LiveBlock({ recap }: { readonly recap: AgentRecap }): ReactNode {
  const { live } = recap;
  if (live === null) {
    return null;
  }
  return (
    <Block title="Live">
      <dl className="grid grid-cols-3 gap-x-3">
        <Stat label="Tokens / min">{formatTokens(live.tokens_per_minute)}</Stat>
        <Stat label="API value / h">
          {/* `api_value_per_hour_usd_micros` is always USD, whatever the display currency. */}
          <Amount micros={live.api_value_per_hour_usd_micros} currency="USD" />
        </Stat>
        <Stat label="Active sessions">{live.active_sessions}</Stat>
      </dl>
    </Block>
  );
}

function ActivityBlock({
  recap,
  ctx,
}: {
  readonly recap: AgentRecap;
  readonly ctx: RecapContext;
}): ReactNode {
  return (
    <Block title="Activity">
      <p className="truncate text-xs text-fg-muted">
        Top model {recap.top_model ?? PLACEHOLDER} · Last activity{" "}
        {recap.last_activity === null
          ? PLACEHOLDER
          : formatClock(Date.parse(recap.last_activity), ctx.timeZone)}
      </p>
    </Block>
  );
}

/**
 * The blocks of one tab. Each is driven by the data it received, never by the agent's name: an
 * agent that cannot report quota has an empty `quota` and gets no quota block (PLAN §7).
 */
export function RecapBlocks(props: {
  readonly recap: AgentRecap;
  readonly ctx: RecapContext;
}): ReactNode {
  const { recap, ctx } = props;
  return (
    <div className="flex flex-col gap-4">
      {recap.quota.length === 0 ? null : (
        <Block title="Quota">
          <QuotaMeters meters={recap.quota} timeZone={ctx.timeZone} now={ctx.nowMs} />
        </Block>
      )}
      <MoneyBlock recap={recap} />
      <BudgetBlock budgets={recap.budgets} />
      <TokensBlock recap={recap} />
      <TrendBlock recap={recap} />
      <LiveBlock recap={recap} />
      <ActivityBlock recap={recap} ctx={ctx} />
    </div>
  );
}
