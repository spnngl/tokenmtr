import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { type Harness, renderApp } from "../../app/app-harness";
import { axeViolations } from "../../shared/axe";
import type { Granularity } from "../../shared/schemas";
import {
  choiceOf,
  defaultGranularity,
  granularityOptions,
  rangeDays,
  rangeOf,
} from "./value-range";

afterEach(cleanup);

const FIGURES = [
  "Paid and API value per day",
  "Cumulative paid and API value",
  "What you paid",
  "Savings",
  "ROI",
] as const;

async function openBilling(options: Harness = {}) {
  const user = userEvent.setup();
  const view = await renderApp("/billing", options);
  await screen.findByRole("figure", { name: "Savings" });
  return { user, ...view };
}

function selected(group: string): string {
  const radios = within(screen.getByRole("radiogroup", { name: group })).getAllByRole("radio");
  return radios.find((radio) => radio.getAttribute("aria-checked") === "true")?.textContent ?? "";
}

function offered(group: string): string[] {
  return within(screen.getByRole("radiogroup", { name: group }))
    .getAllByRole("radio")
    .map((radio) => radio.textContent ?? "");
}

describe("paid against API value range", () => {
  it("maps the global range and picks buckets by length", () => {
    expect(choiceOf({ kind: "preset", preset: "30d" })).toBe("30d");
    expect(choiceOf({ kind: "preset", preset: "7d" })).toBe("filter");
    expect(rangeOf("today", -1, { kind: "preset", preset: "7d" }, "2026-06-15")).toEqual({
      kind: "custom",
      from: "2026-06-14",
      to: "2026-06-14",
    });
    const year = rangeOf("1y", 0, { kind: "preset", preset: "7d" }, "2026-06-15");
    expect(year).toEqual({ kind: "custom", from: "2025-06-16", to: "2026-06-15" });
    expect(rangeDays(year)).toBe(365);
    const defaults: Granularity[] = [1, 92, 93].map(defaultGranularity);
    expect(defaults).toEqual(["hourly", "daily", "monthly"]);
    expect(granularityOptions(1)).toEqual(["hourly", "daily", "monthly"]);
    expect(granularityOptions(401)).toEqual(["monthly"]);
  });
});

describe("paid against API value panel", () => {
  it("draws every chart for the filter range and is accessible", async () => {
    const { container } = await openBilling();
    expect(selected("Range")).toBe("Filter range");
    expect(selected("Granularity")).toBe("Daily");
    for (const name of FIGURES) {
      const figure = screen.getByRole("figure", { name });
      expect(figure.getAttribute("data-points")).toBe("7");
    }
    expect(await axeViolations(container)).toEqual([]);
  });

  it("switches the granularity options with the range", async () => {
    const { user } = await openBilling();
    expect(offered("Granularity")).toEqual(["Daily", "Monthly"]);

    await user.click(screen.getByRole("radio", { name: "Today" }));
    expect(offered("Granularity")).toEqual(["Hourly", "Daily", "Monthly"]);
    expect(selected("Granularity")).toBe("Hourly");
    expect(await screen.findByRole("figure", { name: "Paid and API value per hour" })).toBeTruthy();
    const next = screen.getByRole("button", { name: "Next day" });
    expect(next.hasAttribute("disabled")).toBe(true);
    await user.click(screen.getByRole("button", { name: "Previous day" }));
    expect(next.hasAttribute("disabled")).toBe(false);

    await user.click(screen.getByRole("radio", { name: "1 year" }));
    expect(offered("Granularity")).toEqual(["Daily", "Monthly"]);
    expect(selected("Granularity")).toBe("Monthly");
    expect(
      await screen.findByRole("figure", { name: "Paid and API value per month" }),
    ).toBeTruthy();
  });

  it("splits paid and API value per agent", async () => {
    const asked: boolean[] = [];
    const { user } = await openBilling({
      wrap: (ds) => ({
        ...ds,
        billingSeries: (filters, granularity, split) => {
          asked.push(split);
          return ds.billingSeries(filters, granularity, split);
        },
      }),
    });
    await user.click(screen.getByRole("radio", { name: "By agent" }));
    const figure = await screen.findByRole("figure", { name: "Paid and API value per day" });
    expect(
      await within(figure).findByRole("columnheader", { name: /Unassigned fees/ }),
    ).toBeTruthy();
    expect(asked).toContain(true);
  });

  it("shows each chart as a table on demand", async () => {
    const { user } = await openBilling();
    for (const name of FIGURES) {
      const figure = screen.getByRole("figure", { name });
      const details = figure.querySelector("details");
      expect(details?.open).toBe(false);
      await user.click(within(figure).getByText("View as table"));
      expect(details?.open).toBe(true);
      expect(within(figure).getByRole("table")).toBeTruthy();
    }
  });

  it("says so when nothing was paid or used", async () => {
    await renderApp("/billing", {
      wrap: (ds) => ({
        ...ds,
        billingSeries: async (filters, granularity, split) => {
          const series = await ds.billingSeries(filters, granularity, split);
          return {
            ...series,
            points: series.points.map((point) => ({
              ...point,
              api_value_micros: 0,
              paid_micros: 0,
            })),
          };
        },
      }),
    });
    expect(await screen.findByText("Nothing paid or used")).toBeTruthy();
  });

  it("offers Retry when the series fails", async () => {
    await renderApp("/billing", {
      wrap: (ds) => ({
        ...ds,
        billingSeries: () => Promise.reject(new Error("store unavailable")),
      }),
    });
    const section = await screen.findByRole("region", { name: "Paid against API value" });
    expect(await within(section).findByRole("button", { name: "Retry" })).toBeTruthy();
  });
});
