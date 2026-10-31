// The selectors of the stream store, for `useSelector(stream.store, selector, { compare })`.
// Each one reads a narrow part, so a component renders only when its part changes:
//
// - the structure selector gives the rows: the "quiet" selector, which ignores the text
//   and changes only when a row comes or goes;
// - a part selector gives one message, tool call, approval, steer, prompt or turn end by
//   its key: the reducer keeps the identity of each part until it changes, so only the
//   live row renders while a reply streams, and a settled row never renders again;
// - the status selector gives the running line and the status line;
// - the approvals selector gives the pending approvals of the conversation.
//
// A selector that builds an object has a `compare` function beside it. The stream store
// commits at most once per frame (frame-store.ts), so a selector runs at most once per
// frame too.

import { shallow } from "@tanstack/store";

import type { ContextUse, Mode } from "@/lib/protocol/generated";
import {
  type ApprovalView,
  isQueued,
  type LinkState,
  type MessageView,
  type PromptView,
  pendingApprovals,
  runningTurn,
  type SteerView,
  type StreamState,
  type ToolCallView,
  type TurnEnd,
} from "@/lib/stream/reducer";
import type { TextBuffer } from "@/lib/stream/text-buffer";
import { createRowBuilder, type Row, sameRows } from "./rows";
import { turnStatus } from "./status";
import type { ToolApproval } from "./tool-block";

/** The structure selector of one stream store, with its own memo of rows. */
export function createRowsSelector(): (state: StreamState) => readonly Row[] {
  return createRowBuilder();
}

/** The structure selector of each store: one memo of rows per stream. */
const ROWS_SELECTORS = new WeakMap<object, (state: StreamState) => readonly Row[]>();

/**
 * The structure selector of `store`, the same one on each call. So a view can change
 * its store and keep its state: the rows of each stream keep their own memo.
 */
export function rowsSelectorOf(store: object): (state: StreamState) => readonly Row[] {
  let select = ROWS_SELECTORS.get(store);
  if (select === undefined) {
    select = createRowsSelector();
    ROWS_SELECTORS.set(store, select);
  }
  return select;
}

/** The compare of the structure selector. */
export const compareRows = sameRows;

/** One message of a turn. */
export function selectMessage(
  turnId: string,
  index: number,
): (state: StreamState) => MessageView | undefined {
  return (state) => state.turns.get(turnId)?.messages.get(index);
}

/** One tool call of a turn. */
export function selectTool(
  turnId: string,
  callId: string,
): (state: StreamState) => ToolCallView | undefined {
  return (state) => state.turns.get(turnId)?.tools.get(callId);
}

/** One approval of a turn. */
export function selectApproval(
  turnId: string,
  callId: string,
): (state: StreamState) => ApprovalView | undefined {
  return (state) => state.turns.get(turnId)?.approvals.get(callId);
}

/** The approval question of a tool call, as its tool block needs it. Compare with `shallow`. */
export function selectToolApproval(
  turnId: string,
  callId: string,
): (state: StreamState) => ToolApproval | undefined {
  return (state) => {
    const approval = state.turns.get(turnId)?.approvals.get(callId);
    return approval === undefined
      ? undefined
      : { pending: approval.status === "pending", closedAt: approval.closedAt };
  };
}

/** One steer of a turn. */
export function selectSteer(
  turnId: string,
  seq: number,
): (state: StreamState) => SteerView | undefined {
  return (state) => state.turns.get(turnId)?.steers.get(seq);
}

/** The end of a turn. */
export function selectTurnEnd(turnId: string): (state: StreamState) => TurnEnd | undefined {
  return (state) => state.turns.get(turnId)?.end;
}

/** The prompt of a turn, and whether it waits in the queue. */
export interface PromptRowView {
  readonly prompt: PromptView | undefined;
  readonly queued: boolean;
}

/** The prompt row of a turn. Compare with `shallow`. */
export function selectPrompt(turnId: string): (state: StreamState) => PromptRowView {
  return (state) => {
    const turn = state.turns.get(turnId);
    return {
      prompt: turn?.prompt,
      queued: turn !== undefined && isQueued(turn),
    };
  };
}

/** The thought row of a turn. */
export interface ReasoningRowView {
  readonly buffer: TextBuffer | undefined;
  readonly title: string | undefined;
  /** The turn still runs, so the text can grow. */
  readonly live: boolean;
  readonly startedAt: number | undefined;
}

/** The thought row of a turn. Compare with `shallow`. */
export function selectReasoning(turnId: string): (state: StreamState) => ReasoningRowView {
  return (state) => {
    const turn = state.turns.get(turnId);
    return {
      buffer: turn?.reasoning,
      title: turn?.title,
      live: turn !== undefined && turn.end === undefined,
      startedAt: turn?.startedAt,
    };
  };
}

/** Whether an approval waits, and where it can be answered. */
export type Waiting = "none" | "phone" | "computer";

/** The running line, the composer status line and the state of the stream. */
export interface StatusView {
  /** The turn that runs now. */
  readonly runningTurnId: string | undefined;
  /** When it started, in ms since the epoch, for the elapsed time. */
  readonly runningSince: number | undefined;
  /** What it does now, such as `writing` or `running shell`. */
  readonly activity: string | undefined;
  readonly waiting: Waiting;
  /** How many prompts wait in the queue. */
  readonly queued: number;
  readonly compacting: boolean;
  readonly context: ContextUse | undefined;
  /** The working directory and the mode of the newest turn, else of the summary. */
  readonly cwd: string | undefined;
  readonly mode: Mode | undefined;
  readonly link: LinkState;
  /** True once the cache or efrd gave events. */
  readonly loaded: boolean;
}

/** The status selector. Compare with `shallow`. */
export function selectStatus(state: StreamState): StatusView {
  const running = runningTurn(state);
  let waiting: Waiting = "none";
  for (const approval of running?.approvals.values() ?? []) {
    if (approval.status === "pending") {
      waiting = approval.localOnly ? "computer" : "phone";
      if (waiting === "phone") {
        break;
      }
    }
  }
  let queued = 0;
  let cwd: string | undefined;
  let mode: Mode | undefined;
  for (let i = state.order.length - 1; i >= 0; i--) {
    const turn = state.turns.get(state.order[i] ?? "");
    if (turn === undefined) {
      continue;
    }
    if (isQueued(turn)) {
      queued++;
    }
    cwd ??= turn.cwd;
    mode ??= turn.mode;
  }
  return {
    runningTurnId: running?.turnId,
    runningSince: running?.startedAt,
    activity: running === undefined ? undefined : turnStatus(running, state).text,
    waiting,
    queued,
    compacting: state.compacting,
    context: state.context,
    cwd: cwd ?? state.summary?.cwd ?? undefined,
    mode,
    link: state.link,
    loaded: state.loaded,
  };
}

/** The compare of the status selector and of the other selectors that build an object. */
export const compareShallow: <T>(a: T, b: T) => boolean = shallow;

/** The pending approvals of the conversation, oldest first. */
export function selectPendingApprovals(state: StreamState): readonly ApprovalView[] {
  return pendingApprovals(state);
}

/** The compare of `selectPendingApprovals`: the same approval objects in the same order. */
export function compareApprovals(a: readonly ApprovalView[], b: readonly ApprovalView[]): boolean {
  return a === b || (a.length === b.length && a.every((approval, i) => approval === b[i]));
}
