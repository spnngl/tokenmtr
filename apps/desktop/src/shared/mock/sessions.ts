import type { Filters } from "../filters";
import type { Currency } from "../format";
import type {
  SessionDetail,
  SessionEvent,
  SessionOrigin,
  SessionPage,
  SessionRow,
  SessionToolCall,
} from "../schemas";
import { convert, inRange, matchesProfile, rangeOf, wireUsage } from "./aggregate";
import { createRng, hashString, NOW_MS, SESSIONS, type SessionSeed } from "./seed";

export function iso(ms: number): string {
  return new Date(ms).toISOString().replace(".000Z", "Z");
}

/** Whether `session` ends in the range and passes every other filter. */
export function matches(session: SessionSeed, filters: Filters): boolean {
  const range = rangeOf(filters.range);
  const lastAtMs = session.startedAtMs + session.durationSecs * 1000;
  return (
    inRange(iso(lastAtMs).slice(0, 10), range) &&
    (filters.agent === null || session.agent === filters.agent) &&
    matchesProfile(filters.profile, session.agent, session.profile) &&
    (filters.model === null || session.models.includes(filters.model)) &&
    (filters.project === null || session.project === filters.project) &&
    (filters.client === null || session.client === filters.client) &&
    (filters.billing === "all" || session.billing === filters.billing)
  );
}

function rowOf(session: SessionSeed, currency: Currency): SessionRow {
  return {
    agent: session.agent,
    session: session.session,
    profile: session.profile,
    started_at: iso(session.startedAtMs),
    last_at: iso(session.startedAtMs + session.durationSecs * 1000),
    duration_secs: session.durationSecs,
    project: session.project,
    models: [...session.models],
    tool_calls: session.toolCalls,
    tool_errors: session.toolErrors,
    api_errors: session.apiErrors,
    ...wireUsage(session.usage, currency),
  };
}

export function sessionsOf(filters: Filters, page: number, pageSize: number): SessionPage {
  const matching = SESSIONS.filter((session) => matches(session, filters));
  return {
    currency: filters.currency,
    total: matching.length,
    page,
    page_size: pageSize,
    rows: matching
      .slice(page * pageSize, (page + 1) * pageSize)
      .map((s) => rowOf(s, filters.currency)),
  };
}

export function find(agent: string, session: string): SessionSeed | undefined {
  return SESSIONS.find((candidate) => candidate.agent === agent && candidate.session === session);
}

const TOOLS: readonly { readonly name: string; readonly server: string | null }[] = [
  { name: "Bash", server: null },
  { name: "Read", server: null },
  { name: "Edit", server: null },
  { name: "Grep", server: null },
  { name: "mcp__github__create_issue", server: "github" },
];

const MAX_EVENTS = 60;

export function sessionDetailOf(agent: string, session: string, currency: Currency): SessionDetail {
  const seed = find(agent, session);
  if (seed === undefined) {
    return { currency, events: [], events_truncated: false, tool_calls: [], signals: [] };
  }
  const rng = createRng(hashString(session));
  const count = Math.min(seed.usage.events, MAX_EVENTS);
  const step = (seed.durationSecs * 1000) / Math.max(count, 1);
  const perEvent = Math.round(seed.usage.valueUsd / Math.max(seed.usage.events, 1));
  const events: SessionEvent[] = Array.from({ length: count }, (_, index) => {
    const model = seed.models[index % seed.models.length] ?? seed.models[0] ?? "unknown";
    const output = Math.round(300 + rng() * 1_500);
    return {
      at: iso(seed.startedAtMs + Math.round(step * index)),
      model,
      kind: index % 7 === 6 ? "tool_result" : "message",
      git_branch: seed.project === null ? null : "main",
      total_tokens: Math.round(20_000 + rng() * 60_000),
      output,
      api_value_micros: convert(Math.round(perEvent * (0.5 + rng())), currency),
      cost_source:
        seed.usage.unpriced > 0 ? "unpriced" : seed.usage.reported > 0 ? "reported" : "estimated",
    };
  });
  const toolCalls: SessionToolCall[] = Array.from(
    { length: Math.min(seed.toolCalls, 40) },
    (_, index) => {
      const tool = TOOLS[Math.floor(rng() * TOOLS.length)] ?? TOOLS[0];
      return {
        at: iso(seed.startedAtMs + Math.round((seed.durationSecs * 1000 * index) / 40)),
        tool_name: tool?.name ?? "Bash",
        mcp_server: tool?.server ?? null,
        is_error: rng() < 0.05,
      };
    },
  );
  const signals: SessionDetail["signals"] = Array.from({ length: seed.apiErrors }, (_, index) => [
    iso(seed.startedAtMs + Math.round(seed.durationSecs * 500 * (index + 1))),
    "api_error",
  ]);
  return {
    currency,
    events,
    events_truncated: seed.usage.events > MAX_EVENTS,
    tool_calls: toolCalls,
    signals,
  };
}

const RETENTION_MS = 60 * 86_400_000;

/** Claude deletes transcripts after its retention: old ones are gone. A pi session lives outside the default folder. */
export function sessionOriginOf(agent: string, session: string): SessionOrigin {
  const seed = find(agent, session);
  if (seed === undefined) {
    return { cwd: null, session_dir: null, transcript_path: null, transcript_exists: false };
  }
  const cwd = seed.project === null ? null : `/Users/dev/src/${seed.project}`;
  const exists = !(agent === "claude" && NOW_MS - seed.startedAtMs > RETENTION_MS);
  const customDir = agent === "pi" && hashString(session) % 3 === 0;
  return {
    cwd,
    session_dir: customDir ? "/Volumes/work/pi-sessions" : null,
    transcript_path: exists ? `/Users/dev/.${agent}/sessions/${session}.jsonl` : null,
    transcript_exists: exists,
  };
}
