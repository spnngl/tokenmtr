import { type UseMutationResult, useMutation, useQueryClient } from "@tanstack/react-query";
import { useDataSource } from "./app-context";
import type { DataSource } from "./data-source";

/**
 * A `DataSource` action as a mutation: pending/error state for the button that triggers it, and a
 * refetch of every cached read on success (an action may change anything on screen).
 *
 * `run` must be stable in meaning, not identity: it is called with the data source and the
 * variables passed to `mutate`.
 */
export function useDataAction<Variables = void>(
  run: (dataSource: DataSource, variables: Variables) => Promise<unknown>,
): UseMutationResult<unknown, Error, Variables> {
  const dataSource = useDataSource();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (variables: Variables) => run(dataSource, variables),
    onSuccess: () => queryClient.invalidateQueries(),
  });
}
