import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { type Harness, renderApp } from "../../app/app-harness";
import { axeViolations } from "../../shared/axe";
import { SESSIONS } from "../../shared/mock/seed";

afterEach(cleanup);

const FIGURES = ["Cost over time", "Token mix", "Tool calls"] as const;

/** The busiest seeded session: it has tool calls and a compaction. */
const SEED = SESSIONS.toSorted((a, b) => b.toolCalls - a.toolCalls)[0];

async function openDetail(options: Harness = {}) {
  if (SEED === undefined) {
    throw new Error("no seeded session");
  }
  const view = await renderApp(`/sessions/${SEED.agent}/${SEED.session}`, options);
  await screen.findByRole("heading", { level: 1, name: /^Session / });
  return view;
}

describe("session detail charts", () => {
  it("draws cost over time with signal markers, token mix, cache hits and tools", async () => {
    const { container } = await openDetail();
    for (const name of FIGURES) {
      const figure = await screen.findByRole("figure", { name });
      expect(Number(figure.getAttribute("data-points"))).toBeGreaterThan(0);
      expect(Number(figure.getAttribute("data-points"))).toBeLessThanOrEqual(400);
    }
    expect(screen.getByText("Cache-hit ratio")).toBeTruthy();
    await waitFor(() =>
      expect(container.querySelectorAll(".recharts-reference-line").length).toBeGreaterThan(0),
    );
    expect(await axeViolations(container)).toEqual([]);
  });

  it("shows each chart as a table on demand", async () => {
    const user = userEvent.setup();
    await openDetail();
    for (const name of FIGURES) {
      const figure = await screen.findByRole("figure", { name });
      const details = figure.querySelector("details");
      expect(details?.open).toBe(false);
      await user.click(within(figure).getByText("View as table"));
      expect(details?.open).toBe(true);
      expect(within(figure).getByRole("table")).toBeTruthy();
    }
  });

  it("says so when the session has no priced event or tool call", async () => {
    await openDetail({
      wrap: (ds) => ({
        ...ds,
        sessionTimeline: async (agent, session, currency) => ({
          ...(await ds.sessionTimeline(agent, session, currency)),
          events: [],
          signals: [],
          tools: [],
        }),
      }),
    });
    expect(await screen.findByText("No priced events in this session.")).toBeTruthy();
    expect(screen.getAllByText("No tool calls recorded.").length).toBeGreaterThan(0);
  });

  it("offers Retry when the timeline fails", async () => {
    await openDetail({
      wrap: (ds) => ({
        ...ds,
        sessionTimeline: () => Promise.reject(new Error("store unavailable")),
      }),
    });
    expect(await screen.findByRole("button", { name: "Retry" })).toBeTruthy();
  });
});
