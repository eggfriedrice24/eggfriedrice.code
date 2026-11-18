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
  pendingQuarantine,
  pendingQuestions,
  type PendingWaits,
  pendingWaits,
  type QuarantineQuestionView,
  runningTurn,
  type SteerView,
  type StreamState,
  type ToolCallView,
  type TurnEnd,
  type TurnView,
  type UserQuestionView,
} from "@/lib/stream/reducer";
import type { TextBuffer } from "@/lib/stream/text-buffer";
import type { TurnSettingsView } from "./model-sheet";
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
      : {
          status: approval.status,
          closedAt: approval.closedAt,
          answeredBy: approval.answeredBy,
        };
  };
}

/** When the approval of a tool call stops waiting, in ms since the epoch, while it waits. */
export function selectToolExpiry(
  turnId: string,
  callId: string,
): (state: StreamState) => number | undefined {
  return (state) => {
    const approval = state.turns.get(turnId)?.approvals.get(callId);
    return approval?.status === "pending" ? approval.expiresAt : undefined;
  };
}

/** One steer of a turn. */
export function selectSteer(
  turnId: string,
  seq: number,
): (state: StreamState) => SteerView | undefined {
  return (state) => state.turns.get(turnId)?.steers.get(seq);
}

/** The settings of a turn that names no model: nothing. */
const NO_SETTINGS: TurnSettingsView = {
  model: undefined,
  provider: undefined,
  effort: undefined,
};

/**
 * The model, the account and the effort of the newest turn that names a model: what the
 * conversation runs when the phone sends no choice. Compare with `shallow`.
 */
export function selectLastSettings(state: StreamState): TurnSettingsView {
  for (let i = state.order.length - 1; i >= 0; i--) {
    const turn = state.turns.get(state.order[i] ?? "");
    if (turn?.model !== undefined) {
      return { model: turn.model, provider: turn.provider, effort: turn.effort };
    }
  }
  return NO_SETTINGS;
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
  /** When the first thought ended. */
  readonly endedAt: number | undefined;
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
      endedAt: turn?.thoughtEndedAt,
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

/** True when a quarantine question or a question of the model of `turn` waits. */
function hasPendingQuestion(turn: TurnView): boolean {
  for (const call of turn.tools.values()) {
    if (call.quarantine?.answer === "pending" || call.question?.answer.kind === "pending") {
      return true;
    }
  }
  return false;
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
  // NOTE: the phone can answer Leave to a quarantine question (Q-Quarantine 8) and every
  // question of the model (S-Ask), so such a question waits for the user here too.
  if (waiting === "none" && running !== undefined && hasPendingQuestion(running)) {
    waiting = "phone";
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

/** The quarantine questions that wait, oldest first. Compare with `compareQuarantine`. */
export function selectPendingQuarantine(state: StreamState): readonly QuarantineQuestionView[] {
  return pendingQuarantine(state);
}

/** True when `a` and `b` hold the same objects in the same order. */
function sameItems<T>(a: readonly T[], b: readonly T[]): boolean {
  return a === b || (a.length === b.length && a.every((item, i) => item === b[i]));
}

/** The compare of `selectPendingQuarantine`: the same question objects in the same order. */
export const compareQuarantine: (
  a: readonly QuarantineQuestionView[],
  b: readonly QuarantineQuestionView[],
) => boolean = sameItems;

/** The compare of `selectPendingApprovals`: the same approval objects in the same order. */
export const compareApprovals: (a: readonly ApprovalView[], b: readonly ApprovalView[]) => boolean =
  sameItems;

/** The questions of the model that wait, oldest first. Compare with `compareQuestions`. */
export function selectPendingQuestions(state: StreamState): readonly UserQuestionView[] {
  return pendingQuestions(state);
}

/**
 * The approvals, the quarantine questions and the questions of the model that wait, in
 * one walk of the turns, for a view that shows all three. Compare with
 * `comparePendingWaits`.
 */
export function selectPendingWaits(state: StreamState): PendingWaits {
  return pendingWaits(state);
}

/** The compare of `selectPendingWaits`: the same objects of each kind in the same order. */
export function comparePendingWaits(a: PendingWaits, b: PendingWaits): boolean {
  return (
    a === b ||
    (sameItems(a.approvals, b.approvals) &&
      sameItems(a.quarantine, b.quarantine) &&
      sameItems(a.questions, b.questions))
  );
}

/**
 * The selector of question `questionId` of the model while it waits; undefined for none,
 * and once it ended (the answer state of the composer then ends too).
 */
export function selectWaitingQuestion(
  questionId: string | undefined,
): (state: StreamState) => UserQuestionView | undefined {
  return (state) => {
    if (questionId === undefined) {
      return undefined;
    }
    const running = runningTurn(state);
    for (const call of running?.tools.values() ?? []) {
      if (call.question?.questionId === questionId) {
        return call.question.answer.kind === "pending" ? call.question : undefined;
      }
    }
    return undefined;
  };
}

/** The compare of `selectPendingQuestions`: the same question objects in the same order. */
export const compareQuestions: (
  a: readonly UserQuestionView[],
  b: readonly UserQuestionView[],
) => boolean = sameItems;
