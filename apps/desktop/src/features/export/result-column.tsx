import type { ReactNode } from "react";
import { Badge, type Column } from "../../shared/ui";

/** The "Result" column of a run table: OK, or Failed with the error under it. */
export function resultColumn<T extends { readonly error: string | null }>(): Column<T> {
  return {
    header: "Result",
    cell: (run): ReactNode =>
      run.error === null ? (
        <Badge tone="success">OK</Badge>
      ) : (
        <span className="flex flex-col items-start gap-1">
          <Badge tone="danger">Failed</Badge>
          <span className="text-xs">{run.error}</span>
        </span>
      ),
  };
}
