import { describe, expect, it } from "vitest";
import { defaultFilters, type Filters, parseFilters, serializeFilters } from "./filters";

const defaults = defaultFilters("USD");

describe("parseFilters", () => {
  it("returns the defaults for empty params", () => {
    expect(parseFilters(new URLSearchParams())).toEqual({ filters: defaults, issues: [] });
  });

  it("uses the configured default currency", () => {
    const eur = defaultFilters("EUR");
    expect(parseFilters(new URLSearchParams(), eur).filters.currency).toBe("EUR");
  });

  it("parses every dimension", () => {
    const { filters, issues } = parseFilters(
      new URLSearchParams(
        "range=30d&agent=claude&profile=work&model=opus&project=acme&client=Acme%20Corp&billing=api&currency=EUR",
      ),
    );
    expect(issues).toEqual([]);
    expect(filters).toEqual({
      range: { kind: "preset", preset: "30d" },
      agent: "claude",
      profile: "work",
      model: "opus",
      project: "acme",
      client: "Acme Corp",
      billing: "api",
      currency: "EUR",
    });
  });

  it("parses a custom range", () => {
    const { filters, issues } = parseFilters(
      new URLSearchParams("range=custom&from=2026-03-01&to=2026-03-15"),
    );
    expect(issues).toEqual([]);
    expect(filters.range).toEqual({ kind: "custom", from: "2026-03-01", to: "2026-03-15" });
  });

  it("falls back and reports invalid values", () => {
    const { filters, issues } = parseFilters(
      new URLSearchParams("range=1y&billing=free&currency=GBP&agent="),
    );
    expect(filters).toEqual(defaults);
    expect(issues.map((issue) => issue.param)).toEqual(["range", "agent", "billing", "currency"]);
    expect(issues.find((issue) => issue.param === "range")?.value).toBe("1y");
  });

  it.each([
    ["range=custom", "range"],
    ["range=custom&from=2026-03-01", "range"],
    ["range=custom&from=2026-03-15&to=2026-03-01", "range"],
    ["range=custom&from=2026-02-30&to=2026-03-01", "from"],
  ])("rejects custom range %s", (query, param) => {
    const { filters, issues } = parseFilters(new URLSearchParams(query));
    expect(filters.range).toEqual(defaults.range);
    expect(issues.map((issue) => issue.param)).toEqual([param]);
  });

  it("keeps valid values next to invalid ones", () => {
    const { filters, issues } = parseFilters(new URLSearchParams("agent=pi&billing=nope"));
    expect(filters.agent).toBe("pi");
    expect(filters.billing).toBe("all");
    expect(issues).toHaveLength(1);
  });
});

describe("serializeFilters", () => {
  it("omits defaults", () => {
    expect(serializeFilters(defaults).toString()).toBe("");
  });

  it("round-trips non-default filters", () => {
    const filters: Filters = {
      range: { kind: "custom", from: "2026-03-01", to: "2026-03-15" },
      agent: "claude",
      profile: "work",
      model: "claude-opus-4",
      project: "acme/web",
      client: "Acme Corp & Sons",
      billing: "subscription",
      currency: "EUR",
    };
    expect(parseFilters(serializeFilters(filters))).toEqual({ filters, issues: [] });
  });

  it("round-trips a preset range and a non-USD default", () => {
    const eur = defaultFilters("EUR");
    const filters: Filters = {
      ...eur,
      range: { kind: "preset", preset: "today" },
      currency: "USD",
    };
    const params = serializeFilters(filters, eur);
    expect(params.toString()).toBe("range=today&currency=USD");
    expect(parseFilters(params, eur)).toEqual({ filters, issues: [] });
  });
});
