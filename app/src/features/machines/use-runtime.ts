// The runtime of the machines in React (plan section 2.2, "Runtime in React"). The root
// layout makes the runtime once and gives it through `RuntimeContext`; a screen reads it
// with `useRuntime()`.

import { createContext, use } from "react";

import type { Runtime } from "./model/runtime";

/** The runtime of the machines. The root layout gives it. */
export const RuntimeContext = createContext<Runtime | null>(null);

/** The runtime of the machines. It throws outside the provider of the root layout. */
export function useRuntime(): Runtime {
  const runtime = use(RuntimeContext);
  if (runtime === null) {
    throw new Error("useRuntime needs the RuntimeContext of the root layout");
  }
  return runtime;
}
