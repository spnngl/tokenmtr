import { describe, expect, it } from "vitest";
import { pageCount } from "./sessions-screen";

describe("pageCount", () => {
  it("uses the page size the backend applied", () => {
    expect(pageCount({ total: 101, page_size: 50 })).toBe(3);
    expect(pageCount({ total: 100, page_size: 25 })).toBe(4);
  });

  it("keeps one page when nothing matches", () => {
    expect(pageCount({ total: 0, page_size: 50 })).toBe(1);
    expect(pageCount({ total: 3, page_size: 0 })).toBe(3);
  });
});
