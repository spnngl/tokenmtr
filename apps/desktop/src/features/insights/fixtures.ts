import { micros, usage } from "../../shared/fixtures";
import type { HeatCell } from "../../shared/schemas/usage";
import type { InsightsData, SignalDay } from "./model";

function isoDay(offset: number): string {
  const day = new Date(Date.UTC(2026, 0, 1 + offset));
  return day.toISOString().slice(0, 10);
}

/** `count` consecutive days with a deterministic, bursty pattern. */
export function signalDays(count: number): SignalDay[] {
  return Array.from({ length: count }, (_, index) => ({
    date: isoDay(index),
    api_errors: index % 11 === 0 ? 4 : index % 5 === 0 ? 1 : 0,
    compactions: index % 3 === 0 ? 2 : 1,
  }));
}

const HEAT_CELLS: HeatCell[] = Array.from({ length: 7 }, (_, weekdayIndex) =>
  Array.from({ length: 24 }, (_, hour): HeatCell => {
    const working = weekdayIndex < 5 && hour >= 9 && hour <= 18;
    const events = working ? 20 + ((hour * 7 + weekdayIndex * 3) % 40) : hour === 22 ? 6 : 0;
    return {
      weekday: weekdayIndex + 1,
      hour,
      events,
      total_tokens: events * 12_000,
      api_value_micros: micros(events * 0.35),
    };
  }),
).flat();

/** Tools incl. an MCP tool, MCP servers, signals over 90 days, branches and a full heatmap. */
export const INSIGHTS_DATA: InsightsData = {
  tools: {
    tools: [
      {
        tool_name: "Bash",
        mcp_server: null,
        calls: 1204,
        errors: 96,
        error_rate_percent: 8,
        sessions: 41,
      },
      {
        tool_name: "Read",
        mcp_server: null,
        calls: 980,
        errors: 4,
        error_rate_percent: 0.4,
        sessions: 44,
      },
      {
        tool_name: "Edit",
        mcp_server: null,
        calls: 610,
        errors: 31,
        error_rate_percent: 5.1,
        sessions: 38,
      },
      {
        tool_name: "search_issues",
        mcp_server: "linear",
        calls: 58,
        errors: 9,
        error_rate_percent: 15.5,
        sessions: 7,
      },
      {
        tool_name: "get_page",
        mcp_server: "notion",
        calls: 22,
        errors: 0,
        error_rate_percent: 0,
        sessions: 3,
      },
    ],
    mcp_servers: [
      {
        tool_name: null,
        mcp_server: "linear",
        calls: 58,
        errors: 9,
        error_rate_percent: 15.5,
        sessions: 7,
      },
      {
        tool_name: null,
        mcp_server: "notion",
        calls: 22,
        errors: 0,
        error_rate_percent: 0,
        sessions: 3,
      },
    ],
  },
  signals: {
    days: signalDays(90),
    api_errors: 41,
    compactions: 150,
    top_sessions: [
      { agent: "claude-code", session: "a1b2c3d4", compactions: 7 },
      { agent: "codex", session: "e5f6a7b8", compactions: 3 },
    ],
  },
  branches: {
    currency: "USD",
    rows: [
      {
        branch: "feat/billing",
        ...usage({
          events: 410,
          input_uncached: 900_000,
          output: 200_000,
          api_value_micros: micros(88.4),
          api_value_usd_micros: micros(88.4),
        }),
      },
      {
        branch: "main",
        ...usage({
          events: 220,
          input_uncached: 400_000,
          output: 90_000,
          api_value_micros: micros(31.2),
          api_value_usd_micros: micros(31.2),
        }),
      },
      {
        branch: null,
        ...usage({
          events: 15,
          input_uncached: 20_000,
          output: 4_000,
          api_value_micros: micros(1.1),
          api_value_usd_micros: micros(1.1),
          approximate: true,
        }),
      },
    ],
  },
  heatmap: { currency: "USD", cells: HEAT_CELLS },
};
