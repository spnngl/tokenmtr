import type {
  AgentInfo,
  Alert,
  Backup,
  Dataset,
  FilterOptions,
  FxRateView,
  IngestStatus,
  ParseIssue,
  Settings,
  SinkHealth,
  SystemHealth,
} from "../schemas";
import {
  AGENT_SEEDS,
  addDays,
  EUR_PER_USD,
  type FACTS,
  MOCK_TIME_ZONE,
  NOW_MS,
  TODAY,
} from "./seed";

function iso(ms: number): string {
  return new Date(ms).toISOString().replace(".000Z", "Z");
}

const MINUTE_MS = 60_000;
const HOUR_MS = 3_600_000;

/** `claude` has quota and a subscription, `pi` reports its cost, `aider` has every capability off. */
export function agentInfos(tenYearProfiles: ReadonlySet<string>): AgentInfo[] {
  return [
    {
      agent: "claude",
      label: "Claude Code",
      followed: true,
      capabilities: { quota: true, reported_cost: false, subscription: true },
      profiles: [
        {
          profile: "default",
          root: "/Users/dev/.claude",
          account: "dev@example.com",
          retention_days: 60,
          keeps_ten_years: tenYearProfiles.has("claude:default"),
        },
        {
          profile: "work",
          root: "/Users/dev/.claude-work",
          account: "dev@acme.example",
          retention_days: 30,
          keeps_ten_years: tenYearProfiles.has("claude:work"),
        },
      ],
    },
    {
      agent: "pi",
      label: "pi",
      followed: true,
      capabilities: { quota: false, reported_cost: true, subscription: true },
      profiles: [
        {
          profile: "default",
          root: "/Users/dev/.pi",
          account: null,
          retention_days: null,
          keeps_ten_years: false,
        },
      ],
    },
    {
      agent: "aider",
      label: "aider",
      followed: true,
      capabilities: { quota: false, reported_cost: false, subscription: false },
      profiles: [
        {
          profile: "default",
          root: "/Users/dev/.aider",
          account: null,
          retention_days: null,
          keeps_ten_years: false,
        },
      ],
    },
  ];
}

export function filterOptionsOf(facts: typeof FACTS): FilterOptions {
  const unique = (values: readonly (string | null)[]): string[] =>
    [...new Set(values.filter((value): value is string => value !== null))].sort();
  return {
    agents: unique(facts.map((fact) => fact.agent)),
    profiles: unique(facts.map((fact) => `${fact.agent}/${fact.profile}`)),
    models: unique(facts.map((fact) => fact.model)),
    projects: unique(facts.map((fact) => fact.project)),
    clients: unique(facts.map((fact) => fact.client)),
  };
}

function runs(dataset: Dataset, failing: boolean): SinkHealth["recent_runs"] {
  return [0, 1, 2].map((index) => ({
    dataset,
    started_at: iso(NOW_MS - (index + 1) * 15 * MINUTE_MS),
    finished_at: iso(NOW_MS - (index + 1) * 15 * MINUTE_MS + 4_000),
    rows: failing ? 0 : 120 - index * 10,
    error: failing ? "connection refused (127.0.0.1:5432)" : null,
  }));
}

const SINKS: readonly SinkHealth[] = [
  {
    state: "healthy",
    sink: "local-csv",
    lag_rows: 0,
    lag_secs: 0,
    last_success_at: iso(NOW_MS - 12 * MINUTE_MS),
    recent_runs: runs("usage_events", false),
  },
  {
    state: "failing",
    since: iso(NOW_MS - 3 * HOUR_MS),
    last_error: "connection refused (127.0.0.1:5432)",
    sink: "warehouse",
    lag_rows: 1_240,
    lag_secs: 3 * 3600,
    last_success_at: iso(NOW_MS - 3 * HOUR_MS),
    recent_runs: runs("usage_events", true),
  },
  {
    state: "disabled",
    sink: "sheet",
    lag_rows: 0,
    lag_secs: null,
    last_success_at: null,
    recent_runs: [],
  },
];

export function systemHealthOf(): SystemHealth {
  return {
    level: "error",
    daemon: { state: "running", last_heartbeat: iso(NOW_MS - 5_000), age_secs: 5 },
    sinks: [...SINKS],
    metrics: [],
  };
}

export function ingestStatusOf(): IngestStatus[] {
  return AGENT_SEEDS.map((seed, index) => ({
    agent: seed.agent,
    last_run_at: iso(NOW_MS - 2 * MINUTE_MS),
    files_parsed: 340 - index * 120,
    lines_read: 91_000 - index * 30_000,
    events_found: 12_400 - index * 4_000,
    events_changed: 38 - index * 10,
    open_issues: index === 0 ? 2 : 0,
  }));
}

export const PARSE_ISSUES: readonly ParseIssue[] = [
  {
    agent: "claude",
    kind: "format_drift",
    subject: "usage.server_tool_use",
    occurrences: 14,
    sample:
      '{"type":"assistant","message":{"usage":{"server_tool_use":{"web_search_requests":1}}}}',
    first_seen: iso(NOW_MS - 2 * 86_400_000),
    last_seen: iso(NOW_MS - 3 * HOUR_MS),
    resolved_at: null,
  },
  {
    agent: "claude",
    kind: "missing_cwd",
    subject: "7c1f0b2e-6f3a-4c55-a1d2-0c9e1f6a7b11",
    occurrences: 3,
    sample: null,
    first_seen: iso(NOW_MS - 9 * 86_400_000),
    last_seen: iso(NOW_MS - 8 * 86_400_000),
    resolved_at: iso(NOW_MS - 7 * 86_400_000),
  },
];

export const ALERTS: readonly Alert[] = [
  {
    kind: "quota_threshold",
    window: "five_hour",
    percent: 70,
    fired_at: iso(NOW_MS - 40 * MINUTE_MS),
    dedup_key: "quota:claude:five_hour:70",
  },
  {
    kind: "budget_threshold",
    budget: "weekly-claude-value",
    percent: 100,
    fired_at: iso(NOW_MS - 2 * HOUR_MS),
    dedup_key: "budget:weekly-claude-value:100",
  },
  {
    kind: "sink_failing",
    sink: "warehouse",
    since: iso(NOW_MS - 3 * HOUR_MS),
    fired_at: iso(NOW_MS - 2 * HOUR_MS),
    dedup_key: "sink:warehouse",
  },
];

export const BACKUPS: readonly Backup[] = [
  {
    started_at: iso(NOW_MS - 20 * HOUR_MS),
    finished_at: iso(NOW_MS - 20 * HOUR_MS + 8_000),
    path: "/Users/dev/Library/Application Support/TokenMtr/backups/2026-06-14.db",
    size_bytes: 48_318_464,
    error: null,
  },
  {
    started_at: iso(NOW_MS - 44 * HOUR_MS),
    finished_at: iso(NOW_MS - 44 * HOUR_MS + 2_000),
    path: null,
    size_bytes: null,
    error: "disk full",
  },
];

export function fxRatesOf(): FxRateView[] {
  return Array.from({ length: 30 }, (_, index) => ({
    date: addDays(TODAY, index - 29),
    eur_per_usd_scaled: Math.round((EUR_PER_USD + Math.sin(index) * 0.004) * 1e9),
    source: "ecb",
  }));
}

export function settingsOf(tenYearProfiles: ReadonlySet<string>): Settings {
  return {
    config_path: "/Users/dev/.config/tokenmtr/config.toml",
    exports_dir: "/Users/dev/Documents/TokenMtr",
    machine: "dev-macbook",
    agents: agentInfos(tenYearProfiles),
    daemon: { ...systemHealthOf().daemon, version: "0.3.0" },
    display: { currency: "USD", time_zone: MOCK_TIME_ZONE },
    fx: { fixed_eur_per_usd: null, latest: fxRatesOf().at(-1) ?? null, offline: false },
    pricing: { snapshot_date: "2026-06-01", models: 64 },
    tray: { rings: ["session", "weekly"], hidden_profiles: [], shortcut: "Cmd+Shift+T" },
    git: { default_remote: "origin", default_branch: null },
    alerts: { quota_thresholds: [70, 90], budget_thresholds: [80, 100], quota_reset: true },
    launch_at_login: true,
    protected_folders: [
      { agent: "claude", path: "/Users/dev/.claude", accessible: true },
      { agent: "pi", path: "/Users/dev/Documents/pi", accessible: false },
    ],
    backups: [...BACKUPS],
  };
}
