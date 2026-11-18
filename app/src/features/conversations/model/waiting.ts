// Waits elsewhere: the conversations other than the one on screen that wait for the
// user (plan section 2.2, design-spec section 2): an approval, a question of the model or
// a wait that only the terminal can answer (C-Waiting 0.0.12). They feed the yolk dot on
// the title chevron and the strip under the header. Pure.
//
// Two sources join:
//
// - The recent lists of the connected machines. The watch of the lists of a machine
//   (`conversations.watch`) keeps its list current; for an efrd without the watch, the
//   app polls the list every 10 s (D7). The row state of a list tells each kind of wait.
// - The streams of the conversations in the stack, which win for their conversations
//   because they are live. A stream counts only its approvals. The hook of the app
//   gives no streams today (see `use-waits-elsewhere.ts`), so the lists are the
//   only source.
//
// The conversation on screen never counts: its wait is in its transcript.

import type { ConversationAddress } from "@/lib/query/keys";
import { rowState, waitsForUser, type WaitState } from "@/lib/protocol/status";
import { newestFirst, sameConversation, type ConversationItem } from "./conversation-item";

/** A conversation in the stack, as its stream sees it now. */
export interface LiveConversation {
  readonly machine: string;
  readonly id: string;
  readonly title?: string;
  /** The approvals of the stream that wait for an answer. */
  readonly pendingApprovals: number;
}

/** One conversation that waits for the user, and what it waits for. */
export interface WaitingConversation {
  readonly machine: string;
  readonly id: string;
  readonly title?: string;
  readonly state: WaitState;
}

/**
 * What the strip shows (C-Waiting 0.0.12). `one`: the word of its wait, such as
 * "Question waiting · machine · title", and a tap opens that conversation at its wait.
 * `many`: "N approvals waiting", "N questions waiting" or "N conversations need you",
 * and a tap opens the sheet with its Needs you section on top. `count` counts
 * conversations.
 */
export type WaitsElsewhere =
  | { readonly kind: "none" }
  | { readonly kind: "one"; readonly conversation: WaitingConversation }
  | {
      readonly kind: "many";
      readonly count: number;
      readonly conversations: readonly WaitingConversation[];
    };

/** The sources of the waits elsewhere. */
export interface WaitingInput {
  /** The conversation on screen; undefined on the blank root. */
  readonly current: ConversationAddress | undefined;
  /** The streams of the stacked conversations. */
  readonly live: readonly LiveConversation[];
  /** The recent lists of the connected machines. */
  readonly listed: readonly ConversationItem[];
}

function waitingOf(
  item: { readonly machine: string; readonly id: string; readonly title?: string },
  state: WaitState,
) {
  const waiting: { machine: string; id: string; title?: string; state: WaitState } = {
    machine: item.machine,
    id: item.id,
    state,
  };
  if (item.title !== undefined) {
    waiting.title = item.title;
  }
  return waiting;
}

/** The wait of a live stream: an approval. */
function liveWait(stream: LiveConversation): WaitState | undefined {
  return stream.pendingApprovals > 0 ? "approval" : undefined;
}

/**
 * The wait of a listed conversation, by the row state of the sheet. The phone answers a
 * question of the model on each machine (see the conversations sheet), so a question
 * reads as one that the phone can answer.
 */
function listedWait(item: ConversationItem): WaitState | undefined {
  const state = rowState(item.status, item.attention, true);
  return waitsForUser(state) ? state : undefined;
}

/** The waits elsewhere: newest first by the lists, then the live ones they lack. */
export function waitsElsewhere(input: WaitingInput): WaitsElsewhere {
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
    const state = stream === undefined ? listedWait(item) : liveWait(stream);
    if (state !== undefined && !seen.has(key) && !isCurrent(item)) {
      seen.add(key);
      found.push(waitingOf(stream?.title === undefined ? item : stream, state));
    }
  }
  for (const stream of input.live) {
    const key = `${stream.machine}:${stream.id}`;
    const state = liveWait(stream);
    if (state !== undefined && !seen.has(key) && !isCurrent(stream)) {
      seen.add(key);
      found.push(waitingOf(stream, state));
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
export function showsWaitDot(elsewhere: WaitsElsewhere): boolean {
  return elsewhere.kind !== "none";
}

/** The word of the strip for one conversation that waits in `state`. */
export function waitWord(state: WaitState): string {
  switch (state) {
    case "approval":
      return "Approval waiting";
    case "answer":
      return "Question waiting";
    default:
      return "Answer in the terminal";
  }
}

/**
 * The text of the plural strip: `2 approvals waiting` or `2 questions waiting` when all
 * the waits are of one kind, else `3 conversations need you`. A wait that only the
 * terminal can answer counts as mixed, so a password wait never reads as an approval.
 */
export function manyWaitingText(conversations: readonly WaitingConversation[]): string {
  const count = conversations.length;
  if (conversations.every((conversation) => conversation.state === "approval")) {
    return `${count} approvals waiting`;
  }
  if (conversations.every((conversation) => conversation.state === "answer")) {
    return `${count} questions waiting`;
  }
  return `${count} conversations need you`;
}

/**
 * The words of the waits elsewhere for VoiceOver, after the title of the bar, such as
 * "question waiting" or "3 conversations need you"; undefined when none waits.
 */
export function waitingWords(elsewhere: WaitsElsewhere): string | undefined {
  switch (elsewhere.kind) {
    case "one":
      return waitWord(elsewhere.conversation.state).toLowerCase();
    case "many":
      return manyWaitingText(elsewhere.conversations);
    default:
      return undefined;
  }
}
