// What the All conversations sheet needs from the runtime and the navigation, as one
// small seam. The root layout gives it through `SheetDepsContext`; the tests give fakes.
//
// NOTE: The runtime context and the history helper come from the navigation package of
// the same wave. When they are in the tree, the root layout provides this context from
// them (or this hook reads them directly); the sheet itself does not change.

import type { Readable } from "@tanstack/store";
import { createContext, use } from "react";

import type { RegistryState } from "@/features/machines/model/machine-registry";
import type { ConversationAddress } from "@/lib/query/keys";
import type { MachineCaller } from "@/lib/query/machine-caller";

/** The runtime and the navigation as the sheet uses them. */
export interface SheetDeps {
  /** Asks one machine for its lists (the runtime). */
  readonly caller: MachineCaller;
  /** The paired machines (the registry of the runtime). */
  readonly registry: Readable<RegistryState>;
  /**
   * Opens `ref` after the sheet is dismissed: a push, or a pop back to it when it is in
   * the stack (the history helper). The sheet route is gone from the stack by then.
   */
  readonly open: (ref: ConversationAddress) => void;
  /** The clock of the list times. */
  readonly now: () => number;
}

/** The deps of the sheet; null until the root layout provides them. */
export const SheetDepsContext = createContext<SheetDeps | null>(null);

/** The deps of the sheet. It throws when no provider gives them. */
export function useSheetDeps(): SheetDeps {
  const deps = use(SheetDepsContext);
  if (deps === null) {
    throw new Error("The conversations sheet has no runtime: provide SheetDepsContext.");
  }
  return deps;
}
