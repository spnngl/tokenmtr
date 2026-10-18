/**
 * Zod schemas of every JSON document the UI reads, one module per area. Schemas mirror the serde
 * types of `crates/tokenmtr-query` (snake_case, `*_micros` money in the result's `currency`).
 * Types are always `z.infer`red from them, never written twice.
 */
export * from "./app";
export * from "./filter-spec";
export * from "./health";
export * from "./primitives";
export * from "./quota";
export * from "./recap";
export * from "./usage";
