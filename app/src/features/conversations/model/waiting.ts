// Approvals elsewhere: the conversations other than the one on screen that wait for an
// answer (plan section 2.2, design-spec section 2). They feed the yolk dot on the title
// chevron and the strip under the header. Pure.
//
// Two sources join:
//
// - The streams of the conversations in the stack. They are live, so they win for
//   their conversations.
// - The recent lists of the connected machines. The watch of the lists of a machine
//   (`conversations.watch`) keeps its list current; for an efrd without the watch, the
//   app polls the list every 10 s (D7).
//
// The conversation on screen never counts: its approval is in its transcript.

import type { ConversationAddress } from "@/lib/query/keys";
import { newestFirst, sameConversation, type ConversationItem } from "./conversation-item";

/** A conversation in the stack, as its stream sees it now. */
export interface LiveConversation {
  readonly machine: string;
  readonly id: string;
  readonly title?: string;
  /** The approvals of the stream that wait for an answer. */
  readonly pendingApprovals: number;
}

/** One conversation that waits for an answer. */
export interface WaitingConversation {
  readonly machine: string;
  readonly id: string;
  readonly title?: string;
}

/**
 * What the strip shows. `one`: "Approval waiting · machine · title", and a tap opens
 * that conversation at its approval. `many`: "N approvals waiting", and a tap opens the
 * sheet with the waiting section on top. `count` counts conversations.
 */
export type ApprovalsElsewhere =
  | { readonly kind: "none" }
  | { readonly kind: "one"; readonly conversation: WaitingConversation }
  | {
      readonly kind: "many";
      readonly count: number;
      readonly conversations: readonly WaitingConversation[];
    };

/** The sources of approvals elsewhere. */
export interface WaitingInput {
  /** The conversation on screen; undefined on the blank root. */
  readonly current: ConversationAddress | undefined;
  /** The streams of the stacked conversations. */
  readonly live: readonly LiveConversation[];
  /** The recent lists of the connected machines. */
  readonly listed: readonly ConversationItem[];
}

function waitingOf(item: {
  readonly machine: string;
  readonly id: string;
  readonly title?: string;
}) {
  const waiting: { machine: string; id: string; title?: string } = {
    machine: item.machine,
    id: item.id,
  };
  if (item.title !== undefined) {
    waiting.title = item.title;
  }
  return waiting;
}

/** The approvals elsewhere: newest first by the lists, then the live ones they lack. */
export function approvalsElsewhere(input: WaitingInput): ApprovalsElsewhere {
  const isCurrent = (ref: ConversationAddress): boolean =>
    input.current !== undefined && sameConversation(ref, input.current);
  const live = new Map<string, LiveConversation>();
  for (const conversation of input.live) {
    live.set(`${conversation.machine}:${conversation.id}`, conversation);
  }
  const found: WaitingConversation[] = [];
  const seen = new Set<string>();
  for (const item of [...input.listed].sort(newestFirst)) {
    const key = `${item.machine}:${item.id}`;
    const stream = live.get(key);
    const waits =
      stream === undefined ? item.status === "awaiting_approval" : stream.pendingApprovals > 0;
    if (waits && !seen.has(key) && !isCurrent(item)) {
      seen.add(key);
      found.push(waitingOf(stream?.title === undefined ? item : stream));
    }
  }
  for (const stream of input.live) {
    const key = `${stream.machine}:${stream.id}`;
    if (stream.pendingApprovals > 0 && !seen.has(key) && !isCurrent(stream)) {
      seen.add(key);
      found.push(waitingOf(stream));
    }
  }
  const first = found[0];
  if (first === undefined) {
    return { kind: "none" };
  }
  if (found.length === 1) {
    return { kind: "one", conversation: first };
  }
  return { kind: "many", count: found.length, conversations: found };
}

/** True when the title chevron shows the yolk dot. */
export function showsApprovalDot(elsewhere: ApprovalsElsewhere): boolean {
  return elsewhere.kind !== "none";
}

/** The text of the plural strip: `2 approvals waiting`. */
export function manyApprovalsText(count: number): string {
  return `${count} approvals waiting`;
}
