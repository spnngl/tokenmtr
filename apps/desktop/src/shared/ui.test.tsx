import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { axeViolations } from "./axe";
import { Amount, type Column, DataTable } from "./ui";

afterEach(cleanup);

interface Row {
  readonly name: string;
  readonly cost: number;
}

const ROWS: readonly Row[] = [
  { name: "b", cost: 2 },
  { name: "a", cost: 3 },
  { name: "c", cost: 1 },
];

const COLUMNS: Column<Row>[] = [
  { header: "Name", cell: (row) => row.name, sortValue: (row) => row.name },
  {
    header: "Cost",
    cell: (row) => row.cost,
    sortValue: (row) => row.cost,
    numeric: true,
  },
];

function names(): (string | null)[] {
  return screen
    .getAllByRole("row")
    .slice(1)
    .map((row) => within(row).getAllByRole("cell")[0]?.textContent ?? null);
}

describe("DataTable", () => {
  it("sorts descending, then ascending, then back to the given order", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <DataTable caption="Costs" columns={COLUMNS} rows={ROWS} rowKey={(row) => row.name} />,
    );
    expect(names()).toEqual(["b", "a", "c"]);
    await user.click(screen.getByRole("button", { name: "Cost" }));
    expect(names()).toEqual(["a", "b", "c"]);
    expect(screen.getByRole("columnheader", { name: "Cost" }).getAttribute("aria-sort")).toBe(
      "descending",
    );
    await user.click(screen.getByRole("button", { name: "Cost" }));
    expect(names()).toEqual(["c", "b", "a"]);
    await user.click(screen.getByRole("button", { name: "Cost" }));
    expect(names()).toEqual(["b", "a", "c"]);
    expect(await axeViolations(container)).toEqual([]);
  });
});

describe("Amount", () => {
  it("marks approximate amounts visibly and for screen readers", () => {
    render(<Amount micros={2_350_000} currency="USD" approximate />);
    expect(screen.getByText("$2.35")).toBeTruthy();
    expect(screen.getByText("approximately")).toBeTruthy();
  });

  it("shows exact amounts plainly", () => {
    render(<Amount micros={2_350_000} currency="EUR" />);
    expect(screen.getByText("€2.35")).toBeTruthy();
    expect(screen.queryByText("approximately")).toBeNull();
  });
});
