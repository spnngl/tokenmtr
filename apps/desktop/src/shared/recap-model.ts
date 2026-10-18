import type { IsoDate } from "./format";

/**
 * What the menu-bar recap renders (PLAN §7 recap table). The backend decides which blocks apply
 * to a tab from the agent's capabilities and budgets: an empty `quota` or `budgets` list and a
 * `null` `live` mean the block is not shown. Money is USD micro-units (D7).
 */

export const PERIODS = ["today", "week", "month"] as const;
export type Period = (typeof PERIODS)[number];

/** Id of the tab summing every followed agent. */
export const ALL_TAB_ID = "all";

export type Health = "ok" | "degraded" | "offline";

export interface QuotaWindow {
  readonly id: string;
  /** "5h window", or "Claude 5h window" on the `All` tab. */
  readonly label: string;
  /** 0..1, may exceed 1. */
  readonly usedFraction: number;
  readonly resetsAtMs: number;
  /** When the window hits 100% at the current pace (D33); `null` when it will not. */
  readonly projectedFullAtMs: number | null;
}

export interface MoneyBlock {
  readonly paidUsdMicros: number;
  readonly apiValueUsdMicros: number;
  /** `null` unless a subscription is active. */
  readonly roi: number | null;
  readonly projectedUsdMicros: number | null;
}

export interface BudgetMeter {
  readonly id: string;
  readonly label: string;
  readonly usedFraction: number;
  readonly spentUsdMicros: number;
  readonly projectedUsdMicros: number | null;
}

export interface TokensBlock {
  readonly total: number;
  readonly cachedInputFraction: number;
  readonly output: number;
}

export interface LiveBlock {
  readonly tokensPerMinute: number;
  readonly apiValueUsdMicrosPerHour: number;
  readonly activeSessions: number;
}

export interface ActivityBlock {
  readonly topModel: string | null;
  readonly lastActivityAtMs: number | null;
}

export interface RecapView {
  readonly quota: readonly QuotaWindow[];
  readonly money: MoneyBlock;
  readonly budgets: readonly BudgetMeter[];
  readonly tokens: TokensBlock;
  /** API value of the last 7 days, oldest first. */
  readonly trendUsdMicros: readonly number[];
  readonly live: LiveBlock | null;
  readonly activity: ActivityBlock;
}

export interface RecapTab {
  /** `ALL_TAB_ID` or an agent id. */
  readonly id: string;
  readonly label: string;
  readonly view: RecapView;
}

export interface RecapModel {
  readonly period: Period;
  readonly nowMs: number;
  /** UTC date used to pick the FX rate for aggregate figures (D27). */
  readonly asOf: IsoDate;
  readonly health: Health;
  /** `All` first when more than one agent is followed (D29). Empty when no agent is found. */
  readonly tabs: readonly RecapTab[];
}
