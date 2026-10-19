import { describe, expect, it } from "vitest";
import { staleSince } from "./app-gate";

const READY = { kind: "ready", scan_interval_secs: 60, failed_collectors: 0, os: "macos" } as const;
const NOW = Date.parse("2026-06-15T12:00:00Z");

describe("staleSince", () => {
  it("is null when no scan ever ran", () => {
    expect(staleSince({ ...READY, last_scan_at: null }, NOW)).toBeNull();
  });

  it("is null up to twice the scan interval", () => {
    expect(staleSince({ ...READY, last_scan_at: "2026-06-15T11:58:00Z" }, NOW)).toBeNull();
  });

  it("describes the age beyond twice the scan interval", () => {
    expect(staleSince({ ...READY, last_scan_at: "2026-06-15T11:57:59Z" }, NOW)).toBe("2m 1s ago");
  });
});
