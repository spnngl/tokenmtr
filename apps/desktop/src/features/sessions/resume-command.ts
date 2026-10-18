import type { SessionOrigin } from "../../shared/schemas";

export type ResumeCommand =
  | { readonly kind: "command"; readonly text: string }
  | { readonly kind: "unavailable"; readonly reason: string };

const SAFE = /^[A-Za-z0-9_@%+=:,./-]+$/;

/** POSIX shell quoting: bare when every character is safe, otherwise single-quoted. */
export function shellQuote(value: string): string {
  if (SAFE.test(value)) {
    return value;
  }
  return `'${value.replaceAll("'", "'\\''")}'`;
}

/**
 * The command that resumes a session in its agent. Both agents resolve a session relative
 * to the working directory it was started in, hence the `cd`. pi additionally needs
 * `--session-dir` when the session lives outside its default directory.
 */
export function buildResumeCommand(
  agent: string,
  session: string,
  origin: SessionOrigin,
): ResumeCommand {
  if (agent !== "claude" && agent !== "pi") {
    return { kind: "unavailable", reason: `${agent} has no known resume command.` };
  }
  if (origin.cwd === null) {
    return {
      kind: "unavailable",
      reason: "The session's working directory is unknown, so it cannot be resumed from here.",
    };
  }
  const cd = `cd ${shellQuote(origin.cwd)}`;
  const id = shellQuote(session);
  switch (agent) {
    case "claude":
      return { kind: "command", text: `${cd} && claude --resume ${id}` };
    case "pi": {
      const dir =
        origin.session_dir === null ? "" : ` --session-dir ${shellQuote(origin.session_dir)}`;
      return { kind: "command", text: `${cd} && pi --session ${id}${dir}` };
    }
    default:
      agent satisfies never;
      throw new Error("unreachable agent");
  }
}
