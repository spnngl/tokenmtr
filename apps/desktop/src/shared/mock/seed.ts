/**
 * Deterministic fixture facts behind the mock `DataSource`: ninety days of daily aggregates for
 * three agents (claude with quota and a subscription, pi with reported API cost, aider with every
 * capability off), plus sessions. Same seed, same data, in tests, screenshots and `vite dev`.
 * Money in a fact is USD micros; the mock converts per request.
 */

export const NOW_ISO = "2026-06-15T12:00:00Z";
export const NOW_MS = Date.parse(NOW_ISO);
/** The local day of `NOW_ISO` in `MOCK_TIME_ZONE`. */
export const TODAY = "2026-06-15";
export const MOCK_TIME_ZONE = "Europe/Paris";
const HISTORY_DAYS = 90;
/** Mock EUR per 1 USD. */
export const EUR_PER_USD = 0.92;
const SUBSCRIPTION_FEE_USD_MICROS = 100_000_000;

const DAY_MS = 86_400_000;

export function addDays(date: string, days: number): string {
  return new Date(Date.parse(`${date}T00:00:00Z`) + days * DAY_MS).toISOString().slice(0, 10);
}

/** 1 = Monday .. 7 = Sunday. */
export function weekdayOf(date: string): number {
  const day = new Date(`${date}T00:00:00Z`).getUTCDay();
  return day === 0 ? 7 : day;
}

export type Rng = () => number;

/** mulberry32: small, fast, good enough for fixtures. */
export function createRng(seed: number): Rng {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
  };
}

export function hashString(text: string): number {
  let hash = 2_166_136_261;
  for (let index = 0; index < text.length; index += 1) {
    hash = Math.imul(hash ^ text.charCodeAt(index), 16_777_619);
  }
  return hash >>> 0;
}

export type BillingKind = "subscription" | "api";

interface ModelSeed {
  readonly model: string;
  readonly weight: number;
  /** USD per million tokens = micro-USD per token. */
  readonly input: number;
  readonly output: number;
}

interface ProjectSeed {
  readonly project: string | null;
  readonly client: string | null;
  readonly weight: number;
}

export interface AgentSeed {
  readonly agent: string;
  readonly label: string;
  readonly profile: string;
  readonly billing: BillingKind;
  readonly priced: boolean;
  /** Agent reports its own cost (pi) instead of an estimate from the price table. */
  readonly reportsCost: boolean;
  readonly eventsPerDay: number;
  readonly models: readonly ModelSeed[];
  readonly projects: readonly ProjectSeed[];
}

export const AGENT_SEEDS: readonly AgentSeed[] = [
  {
    agent: "claude",
    label: "Claude Code",
    profile: "default",
    billing: "subscription",
    priced: true,
    reportsCost: false,
    eventsPerDay: 90,
    models: [
      { model: "claude-opus-4", weight: 0.3, input: 15, output: 75 },
      { model: "claude-sonnet-4", weight: 0.55, input: 3, output: 15 },
      { model: "claude-haiku-4", weight: 0.15, input: 1, output: 5 },
    ],
    projects: [
      { project: "tokenmtr", client: null, weight: 0.5 },
      { project: "acme-web", client: "Acme", weight: 0.3 },
      { project: "dotfiles", client: null, weight: 0.12 },
      { project: null, client: null, weight: 0.08 },
    ],
  },
  {
    agent: "pi",
    label: "pi",
    profile: "default",
    billing: "api",
    priced: true,
    reportsCost: true,
    eventsPerDay: 40,
    models: [
      { model: "gpt-5", weight: 0.7, input: 1.25, output: 10 },
      { model: "gemini-2.5-pro", weight: 0.3, input: 1.25, output: 10 },
    ],
    projects: [
      { project: "acme-web", client: "Acme", weight: 0.6 },
      { project: "blog", client: "Personal", weight: 0.4 },
    ],
  },
  {
    agent: "aider",
    label: "aider",
    profile: "default",
    billing: "api",
    priced: false,
    reportsCost: false,
    eventsPerDay: 8,
    models: [{ model: "local-llama", weight: 1, input: 0, output: 0 }],
    projects: [{ project: "blog", client: "Personal", weight: 1 }],
  },
];

/** What every aggregate sums: one agent, model and project on one day. */
export interface RawUsage {
  readonly events: number;
  readonly input_uncached: number;
  readonly cache_read: number;
  readonly cache_write_5m: number;
  readonly cache_write_1h: number;
  readonly output: number;
  readonly reasoning: number;
  readonly valueUsd: number;
  readonly paidUsd: number;
  /** Claude extra usage inside `paidUsd`; only on `EXTRA_FACTS`. */
  readonly extraUsd: number;
  readonly subscriptionValueUsd: number;
  readonly feeUsd: number;
  readonly reported: number;
  readonly estimated: number;
  readonly unpriced: number;
}

export interface Fact extends RawUsage {
  readonly date: string;
  readonly agent: string;
  readonly profile: string;
  readonly model: string;
  readonly project: string | null;
  readonly client: string | null;
  readonly billing: BillingKind;
}

function usageFor(
  seed: AgentSeed,
  model: ModelSeed,
  events: number,
  rng: Rng,
  feeUsd: number,
): RawUsage {
  const jitter = (): number => 0.6 + rng() * 0.8;
  const tokens = {
    input_uncached: Math.round(events * 1_200 * jitter()),
    cache_read: Math.round(events * 38_000 * jitter()),
    cache_write_5m: Math.round(events * 2_500 * jitter()),
    cache_write_1h: Math.round(events * 300 * jitter()),
    output: Math.round(events * 900 * jitter()),
    reasoning: seed.reportsCost ? Math.round(events * 200 * jitter()) : 0,
  };
  const valueUsd = seed.priced
    ? Math.round(
        tokens.input_uncached * model.input +
          tokens.cache_read * model.input * 0.1 +
          tokens.cache_write_5m * model.input * 1.25 +
          tokens.cache_write_1h * model.input * 2 +
          (tokens.output + tokens.reasoning) * model.output,
      )
    : 0;
  const subscribed = seed.billing === "subscription";
  return {
    events,
    ...tokens,
    valueUsd,
    paidUsd: (subscribed ? 0 : valueUsd) + feeUsd,
    extraUsd: 0,
    subscriptionValueUsd: subscribed ? valueUsd : 0,
    feeUsd,
    reported: seed.reportsCost ? events : 0,
    estimated: seed.priced && !seed.reportsCost ? events : 0,
    unpriced: seed.priced ? 0 : events,
  };
}

function buildFacts(): Fact[] {
  const rng = createRng(20_260_615);
  const facts: Fact[] = [];
  for (let back = HISTORY_DAYS - 1; back >= 0; back -= 1) {
    const date = addDays(TODAY, -back);
    const weekend = weekdayOf(date) >= 6;
    for (const seed of AGENT_SEEDS) {
      const base = seed.eventsPerDay * (weekend ? 0.3 : 1) * (0.5 + rng());
      let feeLeft =
        seed.billing === "subscription" ? Math.round(SUBSCRIPTION_FEE_USD_MICROS / 30) : 0;
      for (const model of seed.models) {
        for (const project of seed.projects) {
          const events = Math.round(base * model.weight * project.weight);
          if (events > 0) {
            facts.push({
              date,
              agent: seed.agent,
              profile: seed.profile,
              model: model.model,
              project: project.project,
              client: project.client,
              billing: seed.billing,
              ...usageFor(seed, model, events, rng, feeLeft),
            });
            feeLeft = 0; // the day's fee rides on its first row
          }
        }
      }
    }
  }
  return facts;
}

export const FACTS: readonly Fact[] = buildFacts();

/** The model label of extra-usage rows, as the backend stores them. */
export const EXTRA_USAGE_MODEL = "extra_usage";
/** Monthly cap of claude/default's extra usage. */
export const EXTRA_USAGE_LIMIT_USD_MICROS = 50_000_000;

/**
 * Claude extra usage of claude/default: pay-as-you-go spend on some days, no tokens. Like the
 * backend rows it is billed `api`, has no project, and counts in paid only. Kept apart from
 * `FACTS` so only the views that show it (totals, series, billing, recap) read it.
 */
function buildExtraFacts(): Fact[] {
  const rng = createRng(4_242);
  const facts: Fact[] = [];
  for (let back = HISTORY_DAYS - 1; back >= 0; back -= 1) {
    const date = addDays(TODAY, -back);
    if (weekdayOf(date) <= 5 && rng() < 0.35) {
      const usd = Math.round(400_000 + rng() * 2_600_000);
      facts.push({
        date,
        agent: "claude",
        profile: "default",
        model: EXTRA_USAGE_MODEL,
        project: null,
        client: null,
        billing: "api",
        events: 0,
        input_uncached: 0,
        cache_read: 0,
        cache_write_5m: 0,
        cache_write_1h: 0,
        output: 0,
        reasoning: 0,
        valueUsd: 0,
        paidUsd: usd,
        extraUsd: usd,
        subscriptionValueUsd: 0,
        feeUsd: 0,
        reported: 0,
        estimated: 0,
        unpriced: 0,
      });
    }
  }
  return facts;
}

export const EXTRA_FACTS: readonly Fact[] = buildExtraFacts();

export interface SessionSeed {
  readonly agent: string;
  readonly session: string;
  readonly profile: string;
  readonly startedAtMs: number;
  readonly durationSecs: number;
  readonly project: string | null;
  readonly client: string | null;
  readonly billing: BillingKind;
  readonly models: readonly string[];
  readonly toolCalls: number;
  readonly toolErrors: number;
  readonly apiErrors: number;
  readonly usage: RawUsage;
}

function uuidFrom(rng: Rng): string {
  const hex = (length: number): string =>
    Array.from({ length }, () => Math.floor(rng() * 16).toString(16)).join("");
  return `${hex(8)}-${hex(4)}-4${hex(3)}-a${hex(3)}-${hex(12)}`;
}

function buildSessions(): SessionSeed[] {
  const rng = createRng(7_001);
  const sessions: SessionSeed[] = [];
  for (let index = 0; index < 160; index += 1) {
    const seed = AGENT_SEEDS[Math.floor(rng() * rng() * AGENT_SEEDS.length)];
    if (seed === undefined) {
      continue;
    }
    // Quadratic bias towards recent days.
    const ageMs = Math.floor(rng() * rng() * HISTORY_DAYS * DAY_MS);
    const durationSecs = Math.round(300 + rng() * 14_000);
    const startedAtMs = NOW_MS - ageMs - durationSecs * 1000;
    const first = seed.models[Math.floor(rng() * seed.models.length)];
    const project = seed.projects[Math.floor(rng() * seed.projects.length)];
    if (first === undefined || project === undefined) {
      continue;
    }
    const second = seed.models[Math.floor(rng() * seed.models.length)];
    const events = Math.round(5 + rng() * 120);
    const toolCalls = Math.round(events * (0.5 + rng()));
    sessions.push({
      agent: seed.agent,
      session: uuidFrom(rng),
      profile: seed.profile,
      startedAtMs,
      durationSecs,
      project: project.project,
      client: project.client,
      billing: seed.billing,
      models:
        second === undefined || second.model === first.model
          ? [first.model]
          : [first.model, second.model],
      toolCalls,
      toolErrors: Math.round(toolCalls * rng() * 0.06),
      apiErrors: rng() < 0.12 ? 1 + Math.floor(rng() * 3) : 0,
      usage: usageFor(seed, first, events, rng, 0),
    });
  }
  return sessions.sort(
    (a, b) => b.startedAtMs + b.durationSecs * 1000 - (a.startedAtMs + a.durationSecs * 1000),
  );
}

export const SESSIONS: readonly SessionSeed[] = buildSessions();
