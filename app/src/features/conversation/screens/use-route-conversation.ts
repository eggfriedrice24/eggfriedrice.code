// The conversation that the route of a conversation screen names (model/screen-outbox.ts).
// The first send on the blank root pushes the screen with the local id of a new
// conversation. When efrd made it, the screen follows the real conversation, and its
// route gets the real id (`replaceParams` keeps the screen), so the history rules, the
// back menu and the header see it. The new params drop `enter=instant`: the options of a
// screen also animate its pop, and the back of a first send has the normal animation.

import { useSelector } from "@tanstack/react-store";
import { useSyncExternalStore } from "react";
import {
  useNavigation,
  type NavigationProp,
  type ParamListBase,
} from "expo-router/react-navigation";

import type { ConversationId, MachineId } from "@/features/machines/model/machine-ref";
import {
  type ConversationFocus,
  conversationParams,
  type OpenOptions,
} from "@/features/navigation/model/routes";
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

/**
 * The conversation that the route id `routeId` on `machine` names. `focus` is the focus of
 * the route, which the new params keep.
 */
export function useRouteConversation(
  machine: MachineId,
  routeId: string,
  focus: ConversationFocus | undefined,
): RouteConversationView {
  const outbox = useOutbox();
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const id = useSyncExternalStore(
    (onChange) =>
      followAlias(outbox.store, machine, routeId, (alias) => {
        const options: OpenOptions = focus === undefined ? {} : { focus };
        navigation.replaceParams(conversationParams({ machine, id: alias }, options));
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
