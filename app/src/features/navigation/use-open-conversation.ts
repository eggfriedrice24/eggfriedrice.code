// Opens a conversation by the history rules of model/history.ts (plan section 2.5): the
// conversations sheet, the strip and the first send of the blank root all open a
// conversation here.
//
// - A conversation in the stack: pop back to it.
// - Else: dismiss the sheet on top, drop the oldest conversations above the root when the
//   stack is full (a reset with the kept route keys, so no screen mounts again and nothing
//   animates), then push it. The first send pushes with `enter=instant`.
// - `focus: "approval"` reaches the conversation screen as its `focus` param, also when the
//   screen is in the stack already.
//
// The hook reads the state of the root stack when it runs, not in render, so the screens
// that use it do not render on each navigation.

import { useRouter } from "expo-router";
import {
  CommonActions,
  StackActions,
  useNavigation,
  type NavigationProp,
  type ParamListBase,
} from "expo-router/react-navigation";

import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { planOpen } from "./model/history";
import { conversationHref, stackRouteOf, type OpenOptions } from "./model/routes";

/** Opens `target`. */
export type OpenConversation = (target: ConversationRef, options?: OpenOptions) => void;

/** The function that opens a conversation from the screen of the caller. */
export function useOpenConversation(): OpenConversation {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const router = useRouter();
  return (target, options = {}) => {
    const state = navigation.getState();
    if (state === undefined) {
      router.push(conversationHref(target, options));
      return;
    }
    const plan = planOpen(state.routes.map(stackRouteOf), target, {
      firstSend: options.firstSend === true,
    });
    if (plan.kind === "pop-to") {
      const route = state.routes[plan.index];
      const above = state.routes.length - 1 - plan.index;
      if (above > 0) {
        navigation.dispatch(StackActions.pop(above));
      }
      if (route !== undefined && options.focus !== undefined) {
        navigation.dispatch({
          ...CommonActions.setParams({ focus: options.focus }),
          source: route.key,
        });
      }
      return;
    }
    if (plan.trim > 0) {
      const [root, ...rest] = state.routes;
      if (root !== undefined) {
        const kept = rest.slice(plan.trim, rest.length - plan.dismiss);
        navigation.reset({ ...state, index: kept.length, routes: [root, ...kept], stale: false });
      }
    } else if (plan.dismiss > 0) {
      navigation.dispatch(StackActions.pop(plan.dismiss));
    }
    router.push(conversationHref(target, options));
  };
}
