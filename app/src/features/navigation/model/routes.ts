// The routes of the app (plan section 2.5) and the links to them. Every screen that opens
// another one builds its link here, so the names and the params of the routes are in one
// place. Pure.

import type { Href } from "expo-router";

import type { ConversationRef, MachineId } from "@/features/machines/model/machine-ref";
import type { StackRoute } from "./history";
import { conversationParamsSchema } from "./route-schemas";

/** The names of the routes of the root stack, as expo-router names them. */
export const ROUTE_NAMES = {
  root: "index",
  conversation: "c/[machine]/[id]",
  settings: "settings",
  conversations: "conversations",
  pair: "pair",
} as const;

/** Why a conversation opens: `approval` scrolls it to the approval that waits. */
export type ConversationFocus = "approval";

/** How a conversation opens. */
export interface OpenOptions {
  /** True for the first send on the blank root: the push has no animation. */
  readonly firstSend?: boolean;
  readonly focus?: ConversationFocus;
}

/** The params of a conversation route. */
export interface ConversationParams {
  readonly machine: MachineId;
  readonly id: string;
  readonly enter?: "instant";
  readonly focus?: ConversationFocus;
}

/** The params of the conversation route of `ref`. */
export function conversationParams(
  ref: ConversationRef,
  options: OpenOptions = {},
): ConversationParams {
  const params: { machine: MachineId; id: string; enter?: "instant"; focus?: ConversationFocus } = {
    machine: ref.machine,
    id: ref.id,
  };
  if (options.firstSend === true) {
    params.enter = "instant";
  }
  if (options.focus !== undefined) {
    params.focus = options.focus;
  }
  return params;
}

/** The link to the conversation `ref`. */
export function conversationHref(ref: ConversationRef, options: OpenOptions = {}): Href {
  return { pathname: "/c/[machine]/[id]", params: { ...conversationParams(ref, options) } };
}

/** The link to Settings. */
export const SETTINGS_HREF: Href = "/settings";

/**
 * The link to the All conversations sheet. `waiting` puts the conversations that wait for
 * an answer on top (the strip with two or more approvals, design item 8.6).
 */
export function conversationsHref(section?: "waiting"): Href {
  return section === undefined
    ? "/conversations"
    : { pathname: "/conversations", params: { section } };
}

/**
 * The link to the conversations sheet from the title of a screen on `machine`. The
 * search hits of that machine come first.
 */
export function conversationsSheetHref(machine: MachineId): Href {
  return { pathname: "/conversations", params: { machine } };
}

/** The link to Pairing, or to the re-pair of `machine`. */
export function pairHref(machine?: MachineId): Href {
  return machine === undefined ? "/pair" : { pathname: "/pair", params: { machine } };
}

/** The conversation of the params of a conversation route, or undefined when they are bad. */
export function conversationOfParams(params: unknown): ConversationRef | undefined {
  const parsed = conversationParamsSchema.safeParse(params);
  return parsed.success ? { machine: parsed.data.machine, id: parsed.data.id } : undefined;
}

/** True when the params of a route ask for no push animation (`enter=instant`). */
export function entersInstantly(params: unknown): boolean {
  return (
    typeof params === "object" && params !== null && "enter" in params && params.enter === "instant"
  );
}

/** One route of a navigation state, as react-navigation gives it. */
export interface NavigationRoute {
  readonly name: string;
  readonly params?: object | undefined;
}

/** The route of the history rules for one route of the root stack. */
export function stackRouteOf(route: NavigationRoute): StackRoute {
  if (route.name === ROUTE_NAMES.root) {
    return { name: "index" };
  }
  if (route.name === ROUTE_NAMES.conversation) {
    const ref = conversationOfParams(route.params);
    if (ref !== undefined) {
      return { name: "c", machine: ref.machine, id: ref.id };
    }
  }
  return { name: "other" };
}
