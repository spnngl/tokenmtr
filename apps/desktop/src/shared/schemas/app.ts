import { z } from "zod";
import { CURRENCIES } from "../format";
import { backupSchema, daemonHealthSchema, fxRateSchema } from "./health";
import { countSchema, timestampSchema } from "./primitives";

/**
 * Shapes that no query crate serves: the app shell's own commands (implemented in
 * `apps/desktop/src-tauri`). Query shapes live in `usage.ts`, `quota.ts`, `health.ts`, `recap.ts`.
 */

/** Values the filter bar offers, per dimension. */
export const filterOptionsSchema = z.object({
  agents: z.array(z.string()),
  profiles: z.array(z.string()),
  models: z.array(z.string()),
  projects: z.array(z.string()),
  clients: z.array(z.string()),
});
export type FilterOptions = z.infer<typeof filterOptionsSchema>;

const capabilitiesSchema = z.object({
  quota: z.boolean(),
  reported_cost: z.boolean(),
  subscription: z.boolean(),
});
export type Capabilities = z.infer<typeof capabilitiesSchema>;

/** One profile of an agent, as found on disk. */
const profileInfoSchema = z.object({
  profile: z.string(),
  root: z.string(),
  account: z.string().nullable(),
  /** Claude deletes transcripts after this many days; `null` for agents that do not. */
  retention_days: countSchema.nullable(),
  /** `cleanupPeriodDays` already raised to ten years. */
  keeps_ten_years: z.boolean(),
});
export type ProfileInfo = z.infer<typeof profileInfoSchema>;

const agentInfoSchema = z.object({
  agent: z.string(),
  label: z.string(),
  followed: z.boolean(),
  capabilities: capabilitiesSchema,
  profiles: z.array(profileInfoSchema),
});
export type AgentInfo = z.infer<typeof agentInfoSchema>;

const NOTIFICATION_PERMISSIONS = ["granted", "denied", "undetermined"] as const;

/**
 * Where the app is in its life. Everything but `ready` replaces the screens (first run, daemon
 * offline, version mismatch).
 */
export const appStateSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("ready"),
    last_scan_at: timestampSchema.nullable(),
    scan_interval_secs: countSchema,
    /** Collectors whose last run failed: totals may be incomplete. */
    failed_collectors: countSchema,
  }),
  z.object({
    kind: z.literal("onboarding"),
    agents: z.array(agentInfoSchema),
    daemon_installed: z.boolean(),
    notifications: z.enum(NOTIFICATION_PERMISSIONS),
  }),
  z.object({ kind: z.literal("daemon_offline") }),
  z.object({
    kind: z.literal("incompatible"),
    app_version: z.string(),
    daemon_version: z.string(),
    app_schema: z.number().int().nonnegative(),
    daemon_schema: z.number().int().nonnegative(),
  }),
]);
export type AppState = z.infer<typeof appStateSchema>;

const protectedFolderSchema = z.object({
  agent: z.string(),
  path: z.string(),
  /** macOS blocks reads until the user grants access. */
  accessible: z.boolean(),
});

/** Quota windows the menu bar draws as rings (`menubar.rings`; `MenubarRing` in `tokenmtr-config`). */
export const MENUBAR_RINGS = ["session", "weekly", "per_model"] as const;
export type MenubarRing = (typeof MENUBAR_RINGS)[number];

export const settingsSchema = z.object({
  config_path: z.string(),
  exports_dir: z.string(),
  machine: z.string(),
  agents: z.array(agentInfoSchema),
  daemon: daemonHealthSchema.extend({ version: z.string() }),
  display: z.object({ currency: z.enum(CURRENCIES), time_zone: z.string() }),
  fx: z.object({
    fixed_eur_per_usd: z.number().positive().nullable(),
    latest: fxRateSchema.nullable(),
    offline: z.boolean(),
  }),
  pricing: z.object({ snapshot_date: z.string(), models: countSchema }),
  tray: z.object({
    rings: z.array(z.enum(MENUBAR_RINGS)),
    hidden_profiles: z.array(z.string()),
    shortcut: z.string(),
  }),
  alerts: z.object({
    quota_thresholds: z.array(countSchema),
    budget_thresholds: z.array(countSchema),
    quota_reset: z.boolean(),
  }),
  launch_at_login: z.boolean(),
  protected_folders: z.array(protectedFolderSchema),
  backups: z.array(backupSchema),
});
export type Settings = z.infer<typeof settingsSchema>;

/**
 * What the resume command and the transcript actions need, which session rows do not carry.
 * `session_dir` is set only when the session lives outside the agent's default directory.
 */
export const sessionOriginSchema = z.object({
  cwd: z.string().nullable(),
  session_dir: z.string().nullable(),
  transcript_path: z.string().nullable(),
  transcript_exists: z.boolean(),
});
export type SessionOrigin = z.infer<typeof sessionOriginSchema>;

/**
 * The `[[projects]]` rule the shell suggests for an unresolved directory (`RawProject` in
 * `tokenmtr-config`): `match` is the escaped directory, `name` its repository name when git or
 * Superset knows it, else its folder name.
 */
export const projectRuleSchema = z.object({
  match: z.string(),
  name: z.string(),
  client: z.string().optional(),
});
export type ProjectRule = z.infer<typeof projectRuleSchema>;
