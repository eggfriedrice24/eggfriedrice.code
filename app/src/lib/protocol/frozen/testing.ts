/// <reference types="vite/client" />
// Helpers for the tests of the frozen decoders of released apps: the fixtures of
// efr-protocol by name, and the first problem of a schema with a value. Only test files
// import this module.

import type { z } from "zod";

const FIXTURES = import.meta.glob<unknown>(
  "../../../../../crates/efr-protocol/fixtures/v1/**/*.json",
  { eager: true, import: "default" },
);

/** A JSON object of a fixture. */
export type Json = Readonly<Record<string, unknown>>;

/** True when `value` is a JSON object, not an array or null. */
export function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The fixture `name` under fixtures/v1/, without `.json`. */
export function fixture(name: string): unknown {
  const entry = Object.entries(FIXTURES).find(([path]) => path.endsWith(`/v1/${name}.json`));
  if (entry === undefined) {
    throw new Error(`no fixture ${name}`);
  }
  return entry[1];
}

/** The first problem of `schema` with `value`, or undefined when it passes. */
export function problem(schema: z.ZodType, value: unknown): string | undefined {
  const result = schema.safeParse(value);
  return result.success ? undefined : JSON.stringify(result.error.issues[0]);
}
