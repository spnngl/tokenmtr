import { describe, expect, it } from "vitest";
import type { SessionOrigin } from "../../shared/schemas";
import { buildResumeCommand, shellQuote } from "./resume-command";

const ORIGIN: SessionOrigin = {
  cwd: "/Users/dev/src/tokenmtr",
  session_dir: null,
  transcript_path: "/Users/dev/.claude/projects/x/abc.jsonl",
  transcript_exists: true,
};

describe("buildResumeCommand", () => {
  it("builds the claude command", () => {
    expect(buildResumeCommand("claude", "9f2c-41", ORIGIN)).toEqual({
      kind: "command",
      text: "cd /Users/dev/src/tokenmtr && claude --resume 9f2c-41",
    });
  });

  it("builds the pi command, with --session-dir only outside the default directory", () => {
    expect(buildResumeCommand("pi", "s1", ORIGIN)).toEqual({
      kind: "command",
      text: "cd /Users/dev/src/tokenmtr && pi --session s1",
    });
    expect(
      buildResumeCommand("pi", "s1", { ...ORIGIN, session_dir: "/Volumes/work/pi sessions" }),
    ).toEqual({
      kind: "command",
      text: "cd /Users/dev/src/tokenmtr && pi --session s1 --session-dir '/Volumes/work/pi sessions'",
    });
  });

  it("quotes a working directory with spaces and quotes", () => {
    expect(buildResumeCommand("claude", "s1", { ...ORIGIN, cwd: "/Users/o'neil/My Code" })).toEqual(
      {
        kind: "command",
        text: "cd '/Users/o'\\''neil/My Code' && claude --resume s1",
      },
    );
  });

  it("is unavailable without a working directory", () => {
    const result = buildResumeCommand("claude", "s1", { ...ORIGIN, cwd: null });
    expect(result.kind).toBe("unavailable");
  });

  it("is unavailable for an agent without a resume command", () => {
    expect(buildResumeCommand("aider", "s1", ORIGIN).kind).toBe("unavailable");
  });
});

describe("shellQuote", () => {
  it("leaves safe words bare and quotes everything else", () => {
    expect(shellQuote("a-b_c/1.2")).toBe("a-b_c/1.2");
    expect(shellQuote("a b")).toBe("'a b'");
    expect(shellQuote("$HOME")).toBe("'$HOME'");
    expect(shellQuote("")).toBe("''");
  });
});
