// What the All conversations sheet needs from the runtime and the navigation, as one
// small seam. The sheet reads the runtime from the services of the root layout and opens
// a conversation with the history helper of the navigation feature. A test can give
// other deps through `SheetDepsContext`.

import type { Readable } from "@tanstack/store";
import { useRouter } from "expo-router";
import { createContext, use } from "react";

import type { RegistryState } from "@/features/machines/model/machine-registry";
import { AppServicesContext } from "@/features/navigation/use-app-services";
import { useOpenConversation } from "@/features/navigation/use-open-conversation";
import type { ConversationAddress } from "@/lib/query/keys";
import type { MachineCaller } from "@/lib/query/machine-caller";

/** The runtime and the navigation as the sheet uses them. */
export interface SheetDeps {
  /** Asks one machine for its lists (the runtime). */
  readonly caller: MachineCaller;
  /** The paired machines (the registry of the runtime). */
  readonly registry: Readable<RegistryState>;
  /**
   * Dismisses the sheet and opens `ref`: a push, or a pop back to it when it is in the
   * stack (the history helper).
   */
  readonly open: (ref: ConversationAddress) => void;
  /** Dismisses the sheet and goes back to the blank root, for a new conversation. */
  readonly newConversation: () => void;
  /** The clock of the list times. */
  readonly now: () => number;
}

/** Deps that replace the ones of the app, for the tests. */
export const SheetDepsContext = createContext<SheetDeps | null>(null);

/**
 * The deps of the sheet: the ones of `SheetDepsContext`, else the runtime of the root
 * layout and the history helper. It throws when neither is there.
 */
export function useSheetDeps(): SheetDeps {
  const injected = use(SheetDepsContext);
  const services = use(AppServicesContext);
  const openConversation = useOpenConversation();
  const router = useRouter();
  if (injected !== null) {
    return injected;
  }
  if (services === null) {
    throw new Error("The conversations sheet has no runtime: it needs the root layout.");
  }
  return {
    caller: services.runtime,
    registry: services.runtime.registry,
    open: (ref) => openConversation(ref),
    // The sheet is on the stack, so this pops it and every conversation.
    newConversation: () => router.dismissAll(),
    now: Date.now,
  };
}
