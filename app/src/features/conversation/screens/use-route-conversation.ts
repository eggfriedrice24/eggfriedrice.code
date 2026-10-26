// The conversation that the route of a conversation screen names (model/screen-outbox.ts).
// The first send on the blank root pushes the screen with the local id of a new
// conversation. When efrd made it, the screen follows the real conversation, and its
// route gets the real id (`setParams` keeps the screen), so the history rules, the back
// menu and the header see it.

import { useSelector } from "@tanstack/react-store";
import { useSyncExternalStore } from "react";
import {
  useNavigation,
  type NavigationProp,
  type ParamListBase,
} from "expo-router/react-navigation";

import type { ConversationId, MachineId } from "@/features/machines/model/machine-ref";
import { useOutbox } from "@/features/navigation/use-app-services";
import { resolveAlias } from "../model/outbox";
import { followAlias, routeConversation } from "../model/screen-outbox";

/** The conversation of a route. */
export interface RouteConversationView {
  /** The conversation id: the one of efrd, or the local id while efrd has not made it. */
  readonly id: ConversationId;
  /** True while efrd has not made the conversation: the screen has no stream yet. */
  readonly pending: boolean;
}

/** The conversation that the route id `routeId` on `machine` names. */
export function useRouteConversation(machine: MachineId, routeId: string): RouteConversationView {
  const outbox = useOutbox();
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const id = useSyncExternalStore(
    (onChange) =>
      followAlias(outbox.store, machine, routeId, (alias) => {
        navigation.setParams({ id: alias });
        onChange();
      }),
    () => resolveAlias(outbox.store.get(), machine, routeId) ?? routeId,
  );
  const pending = useSelector(
    outbox.store,
    (snapshot) => routeConversation(snapshot, machine, routeId).kind === "new",
  );
  return { id, pending };
}
