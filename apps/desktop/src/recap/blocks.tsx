import type { ReactNode } from "react";
import {
  type Currency,
  type Fx,
  formatClock,
  formatPercent,
  formatRelativeReset,
  formatTokens,
  type IsoDate,
} from "../shared/format";
import { Money } from "../shared/money";
import type { RecapView } from "../shared/recap-model";
import { UsageMeter } from "../shared/usage-meter";

/** What every block needs to format its figures. */
export interface RecapContext {
  readonly nowMs: number;
  readonly asOf: IsoDate;
  readonly currency: Currency;
  readonly fx: Fx;
  readonly timeZone: string;
}

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

function QuotaBlock({
  view,
  ctx,
}: {
  readonly view: RecapView;
  readonly ctx: RecapContext;
}): ReactNode {
  return (
    <Block title="Quota">
      {view.quota.map((window) => (
        <UsageMeter
          key={window.id}
          label={window.label}
          usedFraction={window.usedFraction}
          detail={
            window.projectedFullAtMs === null
              ? formatRelativeReset(window.resetsAtMs, ctx.nowMs)
              : `${formatRelativeReset(window.resetsAtMs, ctx.nowMs)} · 100% at ${formatClock(window.projectedFullAtMs, ctx.timeZone)} at this pace`
          }
        />
      ))}
    </Block>
  );
}

function MoneyBlock({
  view,
  ctx,
}: {
  readonly view: RecapView;
  readonly ctx: RecapContext;
}): ReactNode {
  const { money } = view;
  const usd = (usdMicros: number): ReactNode => (
    <Money usdMicros={usdMicros} date={ctx.asOf} currency={ctx.currency} fx={ctx.fx} />
  );
  return (
    <Block title="Money">
      <dl className="grid grid-cols-2 gap-x-3 gap-y-2">
        <Stat label="Paid" lead>
          {usd(money.paidUsdMicros)}
        </Stat>
        <Stat label="API value" lead>
          {usd(money.apiValueUsdMicros)}
        </Stat>
        {money.roi === null ? null : <Stat label="ROI">{`${money.roi.toFixed(1)}x`}</Stat>}
        {money.projectedUsdMicros === null ? null : (
          <Stat label="Projected">{usd(money.projectedUsdMicros)}</Stat>
        )}
      </dl>
    </Block>
  );
}

function BudgetBlock({
  view,
  ctx,
}: {
  readonly view: RecapView;
  readonly ctx: RecapContext;
}): ReactNode {
  return (
    <Block title="Budgets">
      {view.budgets.map((budget) => (
        <UsageMeter
          key={budget.id}
          label={budget.label}
          usedFraction={budget.usedFraction}
          detail={
            <>
              <Money
                usdMicros={budget.spentUsdMicros}
                date={ctx.asOf}
                currency={ctx.currency}
                fx={ctx.fx}
              />{" "}
              spent
              {budget.projectedUsdMicros === null ? null : (
                <>
                  {" · projected "}
                  <Money
                    usdMicros={budget.projectedUsdMicros}
                    date={ctx.asOf}
                    currency={ctx.currency}
                    fx={ctx.fx}
                  />
                </>
              )}
            </>
          }
        />
      ))}
    </Block>
  );
}

function TokensBlock({ view }: { readonly view: RecapView }): ReactNode {
  return (
    <Block title="Tokens">
      <dl className="grid grid-cols-3 gap-x-3">
        <Stat label="Tokens">{formatTokens(view.tokens.total)}</Stat>
        <Stat label="Cached input">{formatPercent(view.tokens.cachedInputFraction)}</Stat>
        <Stat label="Output">{formatTokens(view.tokens.output)}</Stat>
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

function TrendBlock({
  view,
  ctx,
}: {
  readonly view: RecapView;
  readonly ctx: RecapContext;
}): ReactNode {
  const { trendUsdMicros } = view;
  const peak = Math.max(...trendUsdMicros, 0);
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
            points={sparklinePoints(trendUsdMicros)}
            fill="none"
            strokeWidth={2}
            vectorEffect="non-scaling-stroke"
            className="stroke-chart-1"
          />
        </svg>
        <span className="text-xs text-fg-muted">
          7-day peak <Money usdMicros={peak} date={ctx.asOf} currency={ctx.currency} fx={ctx.fx} />
        </span>
      </div>
    </Block>
  );
}

function LiveBlock({
  view,
  ctx,
}: {
  readonly view: RecapView;
  readonly ctx: RecapContext;
}): ReactNode {
  const { live } = view;
  if (live === null) {
    return null;
  }
  return (
    <Block title="Live">
      <dl className="grid grid-cols-3 gap-x-3">
        <Stat label="Tokens / min">{formatTokens(live.tokensPerMinute)}</Stat>
        <Stat label="API value / h">
          <Money
            usdMicros={live.apiValueUsdMicrosPerHour}
            date={ctx.asOf}
            currency={ctx.currency}
            fx={ctx.fx}
          />
        </Stat>
        <Stat label="Active sessions">{live.activeSessions}</Stat>
      </dl>
    </Block>
  );
}

function ActivityBlock({
  view,
  ctx,
}: {
  readonly view: RecapView;
  readonly ctx: RecapContext;
}): ReactNode {
  const { topModel, lastActivityAtMs } = view.activity;
  return (
    <Block title="Activity">
      <p className="truncate text-xs text-fg-muted">
        Top model {topModel ?? "\u2014"} · Last activity{" "}
        {lastActivityAtMs === null ? "\u2014" : formatClock(lastActivityAtMs, ctx.timeZone)}
      </p>
    </Block>
  );
}

/** The §7 recap blocks for one tab. Optional blocks vanish when their data is empty. */
export function RecapBlocks({
  view,
  ctx,
}: {
  readonly view: RecapView;
  readonly ctx: RecapContext;
}): ReactNode {
  return (
    <div className="flex flex-col gap-4">
      {view.quota.length === 0 ? null : <QuotaBlock view={view} ctx={ctx} />}
      <MoneyBlock view={view} ctx={ctx} />
      {view.budgets.length === 0 ? null : <BudgetBlock view={view} ctx={ctx} />}
      <TokensBlock view={view} />
      <TrendBlock view={view} ctx={ctx} />
      <LiveBlock view={view} ctx={ctx} />
      <ActivityBlock view={view} ctx={ctx} />
    </div>
  );
}
