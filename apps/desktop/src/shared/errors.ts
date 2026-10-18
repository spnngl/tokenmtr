import { z } from "zod";
import { ERROR_KINDS } from "./bindings";
import { APP_NAME } from "./brand";

/** Class of a failed shell command (`ErrorKind` in `src-tauri/src/error.rs`). */
export type CommandErrorKind = (typeof ERROR_KINDS)[number];

/**
 * Why a `DataSource` call failed, tagged by `kind`.
 * - a `CommandErrorKind`: the shell answered with a `CommandError`; `message` is its sentence for
 *   the user.
 * - `schema`: the answer does not match its Zod schema (Rust/TypeScript drift). `detail` is for
 *   the console only, never for the user.
 * - `unknown`: anything else, including a `CommandError` kind newer than this UI. `message` is
 *   kept so new kinds degrade to their text.
 */
export type DataSourceFailure =
  | { readonly kind: CommandErrorKind; readonly message: string }
  | { readonly kind: "schema"; readonly detail: string }
  | { readonly kind: "unknown"; readonly message: string };

/** A failed `DataSource` call. `message` is the user-facing sentence of `failure`. */
export class DataSourceError extends Error {
  override readonly name = "DataSourceError";
  readonly command: string;
  readonly failure: DataSourceFailure;

  constructor(command: string, failure: DataSourceFailure) {
    super(failureText(failure).message);
    this.command = command;
    this.failure = failure;
  }
}

/** A failed command as the shell serialises it; `kind` stays open so newer kinds still parse. */
const commandErrorSchema = z.object({ kind: z.string(), message: z.string() });
const knownKindSchema = z.enum(ERROR_KINDS);

/** Classifies whatever `invoke` rejected with. */
export function commandFailure(error: unknown): DataSourceFailure {
  const parsed = commandErrorSchema.safeParse(error);
  if (parsed.success) {
    const kind = knownKindSchema.safeParse(parsed.data.kind);
    return kind.success
      ? { kind: kind.data, message: parsed.data.message }
      : { kind: "unknown", message: parsed.data.message };
  }
  if (error instanceof Error) {
    return { kind: "unknown", message: error.message };
  }
  return { kind: "unknown", message: typeof error === "string" ? error : "No details." };
}

/** What the user reads about a failure: a short title and a sentence. */
export interface FailureText {
  readonly title: string;
  readonly message: string;
}

const TITLES: Record<CommandErrorKind, string> = {
  invalid_request: "Request not valid",
  daemon_offline: "Daemon not running",
  daemon_incompatible: "Update required",
  daemon_running: "Daemon still running",
  remote: "The daemon refused the request",
  store: "Could not read the usage database",
  config: "Config problem",
  io: "File operation failed",
  service: "Background service problem",
  app_not_installed: `Move ${APP_NAME} to Applications`,
  internal: "Something went wrong",
};

/** The title and sentence shown for `failure`. */
function failureText(failure: DataSourceFailure): FailureText {
  switch (failure.kind) {
    case "schema":
      return {
        title: "Unexpected data",
        message: "The app and its interface disagree on a data format. Update the app.",
      };
    case "unknown":
      return { title: "Something went wrong", message: failure.message };
    default:
      return { title: TITLES[failure.kind], message: failure.message };
  }
}

/** The failure kind of any thrown value, classified like `errorText` does. */
export function failureKind(error: unknown): DataSourceFailure["kind"] {
  return error instanceof DataSourceError ? error.failure.kind : commandFailure(error).kind;
}

/** The text of any thrown value: `DataSourceError`s by kind, other errors by their message. */
export function errorText(error: unknown): FailureText {
  if (error instanceof DataSourceError) {
    return failureText(error.failure);
  }
  return failureText(commandFailure(error));
}
