// What an approval block shows in each state of the design (3.11 and the board
// S-Approvals), on the one approval type of the stream reducer. Pure, so each state has a
// test; approval-block.tsx only draws the model.
//
// - Open: the gold outline, the header, the command (or the edit line and the diff), the
//   cwd and the optional reason, then Deny and Allow. Allow shows the braille cell while
//   the answer is on its way, and until efrd sends `approval_resolved`, because efrd's
//   first answer wins.
// - Answer in the terminal: efrd sets `local_only` for a privilege exit or a command that
//   runs as another user, for a background job and for a delete that full mode guards
//   (crates/efr-conversation/src/turn.rs). The phone gets no buttons; a line names the
//   machine where the user can answer, and a sentence says why.
// - An "always" rule: efrd takes Allow and Deny from a phone, and refuses only
//   `allow_always` (docs/protocol.md, "Remote devices"). So the block keeps its buttons,
//   and the line tells that the rule is set in the terminal. The board draws this case
//   with no buttons; that would take an answer away that efrd allows.
// - Closed: a quiet line with a glyph and the time, then the command on one line.

import { ConnectionError, ServerError } from "@/lib/net/errors";
import type { ApprovalView } from "@/lib/stream/reducer";
import { shortPath } from "@/features/conversations/model/list-format";
import { isApprovalAnswer, type PhoneDecision } from "../conversation-queries";
import { localReason } from "./approval";
import { diffLines, diffStat, type DiffLine } from "./diff-lines";
import { clockTime } from "./durations";
import {
  reasonSpans,
  subjectLine,
  summaryParts,
  TIME_UP,
  type ReasonSpan,
  type Subject,
} from "./approval-block-text";

/** Why the block sends the user to the terminal. */
export type TerminalReason = "privilege" | "delete" | "job" | "always";

/**
 * Why the terminal must answer `approval`, or must set its rule; `undefined` when the
 * phone can do all that the question offers. Only `always` keeps the buttons.
 */
export function terminalReason(approval: ApprovalView): TerminalReason | undefined {
  const local = localReason(approval);
  if (local !== undefined) {
    return local;
  }
  return approval.always === undefined ? undefined : "always";
}

/** The sentence under the terminal line, for each reason (the board's words). */
export const TERMINAL_NOTES: Readonly<Record<TerminalReason, string>> = {
  privilege: "It needs your password, which only the terminal can take.",
  delete: "Full mode guards this delete, so only the terminal can approve it.",
  job: "It keeps running after the turn ends, so efr only starts it from the terminal.",
  always: 'Answering can add an "always" rule, and rules are only set in the terminal.',
};

/** The terminal line: `answer on desktop`, or `always rule on desktop`. */
export function terminalLine(reason: TerminalReason, machine: string): string {
  return reason === "always" ? `always rule on ${machine}` : `answer on ${machine}`;
}

/** One part of the code of a diff row: the changed run of a paired line is strong. */
export interface DiffRun {
  /** The offset of the run in its line, its key. */
  readonly at: number;
  readonly text: string;
  readonly strong: boolean;
}

/** One row of the diff box of a file write. */
export interface DiffRow {
  /** The key of the row: its place in the box. */
  readonly key: string;
  readonly kind: "added" | "removed" | "context" | "hunk" | "note" | "other";
  /** The number of the gutter: the new line, or the old one for a removed line. */
  readonly number: string;
  /** `+`, `-`, a space, or "" for a row that is not a line of code. */
  readonly sign: string;
  readonly runs: readonly DiffRun[];
}

/**
 * The rows of the diff box of `diff`: the lines of its hunks, without the file headers.
 * A hunk header shows only when the diff has more than one hunk. When a block of removed
 * lines comes right before a block of added lines, the lines pair up in order, and the
 * part that changed is strong (design 3.8: only when a removed and an added line pair up).
 */
export function approvalDiffRows(diff: string): DiffRow[] {
  const lines = diffLines(diff).filter((line) => line.kind !== "file" && line.kind !== "meta");
  const hunks = lines.filter((line) => line.kind === "hunk").length;
  const rows: DiffRow[] = [];
  const add = (row: Omit<DiffRow, "key">): void => {
    rows.push({ key: String(rows.length), ...row });
  };
  for (let at = 0; at < lines.length;) {
    const line = lines[at];
    if (line === undefined) {
      break;
    }
    if (line.kind === "removed") {
      const removed = runOf(lines, at, "removed");
      const added = runOf(lines, at + removed.length, "added");
      for (const [index, old] of removed.entries()) {
        const pair = added[index];
        add(codeRow(old, pair === undefined ? undefined : changed(codeOf(old), codeOf(pair))?.[0]));
      }
      for (const [index, now] of added.entries()) {
        const pair = removed[index];
        add(codeRow(now, pair === undefined ? undefined : changed(codeOf(pair), codeOf(now))?.[1]));
      }
      at += removed.length + added.length;
      continue;
    }
    at += 1;
    switch (line.kind) {
      case "hunk":
        if (hunks > 1) {
          add({ kind: "hunk", number: "", sign: "", runs: plain(line.text) });
        }
        break;
      case "added":
      case "context":
        add(codeRow(line, undefined));
        break;
      case "note":
        add({ kind: "note", number: "", sign: "", runs: plain(line.text) });
        break;
      default:
        add({ kind: "other", number: "", sign: "", runs: plain(line.text) });
        break;
    }
  }
  return rows;
}

/** The lines of `kind` from `start` on, up to the first line of another kind. */
function runOf(lines: readonly DiffLine[], start: number, kind: DiffLine["kind"]): DiffLine[] {
  const run: DiffLine[] = [];
  for (let at = start; lines[at]?.kind === kind; at++) {
    const line = lines[at];
    if (line !== undefined) {
      run.push(line);
    }
  }
  return run;
}

function codeOf(line: DiffLine): string {
  return line.text.slice(1);
}

function plain(text: string): DiffRun[] {
  return [{ at: 0, text, strong: false }];
}

function codeRow(line: DiffLine, runs: readonly DiffRun[] | undefined): Omit<DiffRow, "key"> {
  const kind = line.kind === "removed" ? "removed" : line.kind === "added" ? "added" : "context";
  const number = kind === "removed" ? line.oldLine : (line.newLine ?? line.oldLine);
  return {
    kind,
    number: number === undefined ? "" : String(number),
    sign: line.text.charAt(0) || " ",
    runs: runs ?? plain(codeOf(line)),
  };
}

/**
 * The runs of an old and a new line: the common start and end stay plain, the middle
 * is strong. `undefined` when the lines share nothing, so a replaced line is not one
 * strong run.
 */
export function changed(
  old: string,
  now: string,
): readonly [readonly DiffRun[], readonly DiffRun[]] | undefined {
  let start = 0;
  while (start < old.length && start < now.length && old[start] === now[start]) {
    start++;
  }
  // NOTE: do not cut a surrogate pair in two.
  if (start > 0 && isHighSurrogate(old.charCodeAt(start - 1))) {
    start--;
  }
  let end = 0;
  while (
    end < old.length - start &&
    end < now.length - start &&
    old[old.length - 1 - end] === now[now.length - 1 - end]
  ) {
    end++;
  }
  if (end > 0 && isLowSurrogate(old.charCodeAt(old.length - end))) {
    end--;
  }
  if (start + end === 0) {
    return undefined;
  }
  return [split(old, start, end), split(now, start, end)];
}

function split(text: string, start: number, end: number): DiffRun[] {
  const runs: DiffRun[] = [];
  const middle = text.slice(start, text.length - end);
  if (start > 0) {
    runs.push({ at: 0, text: text.slice(0, start), strong: false });
  }
  if (middle.length > 0) {
    runs.push({ at: start, text: middle, strong: true });
  }
  if (end > 0) {
    runs.push({ at: text.length - end, text: text.slice(text.length - end), strong: false });
  }
  return runs;
}

function isHighSurrogate(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/** One answer of this phone, as `useMutationState` gives it. */
export interface AnswerMutation {
  readonly variables: unknown;
  readonly status: "idle" | "pending" | "success" | "error";
  readonly error: unknown;
  readonly submittedAt: number;
}

/** What this phone did about one question. */
export interface AnswerState {
  /** The answer on its way, or sent and not yet confirmed by `approval_resolved`. */
  readonly sending: PhoneDecision | undefined;
  /** True when efrd took an answer of this phone. */
  readonly answeredHere: boolean;
  /** The newest answer, when it failed: its error and its variables for a resend. */
  readonly failed:
    | {
        readonly error: unknown;
        readonly decision: PhoneDecision;
        readonly commandId: string;
      }
    | undefined;
}

/** The state of no answer. */
export const NO_ANSWER: AnswerState = {
  sending: undefined,
  answeredHere: false,
  failed: undefined,
};

/**
 * What this phone did about question `callId`, from the answers of its conversation.
 * `open` is true while the question is still pending in the stream.
 */
export function answerState(
  mutations: readonly AnswerMutation[],
  callId: string,
  open: boolean,
): AnswerState {
  let newest:
    | {
        readonly mutation: AnswerMutation;
        readonly decision: PhoneDecision;
        readonly commandId: string;
      }
    | undefined;
  let answeredHere = false;
  for (const mutation of mutations) {
    const variables = mutation.variables;
    if (!isApprovalAnswer(variables) || variables.callId !== callId) {
      continue;
    }
    if (mutation.status === "success") {
      answeredHere = true;
    }
    if (newest === undefined || mutation.submittedAt >= newest.mutation.submittedAt) {
      newest = { mutation, decision: variables.decision, commandId: variables.commandId };
    }
  }
  if (newest === undefined) {
    return NO_ANSWER;
  }
  const status = newest.mutation.status;
  const sending =
    status === "pending" || (status === "success" && open) ? newest.decision : undefined;
  const failed =
    status === "error" && open
      ? { error: newest.mutation.error, decision: newest.decision, commandId: newest.commandId }
      : undefined;
  return { sending, answeredHere, failed };
}

/** The sentence of a failed answer. */
export function answerErrorText(error: unknown, machine: string): string {
  if (error instanceof ServerError) {
    switch (error.body.code) {
      case "forbidden":
        return `efrd takes no answer to this question from a phone. Answer on ${machine}.`;
      case "not_found":
      case "conflict":
        return "The question is not open now.";
      default:
        return "efrd did not take the answer. Try again.";
    }
  }
  if (error instanceof ConnectionError) {
    return `The answer did not reach ${machine}. Try again.`;
  }
  return "The answer failed. Try again.";
}

/** What the block knows besides the question. */
export interface ApprovalBlockContext {
  /** The short name of the machine, such as `desktop`. */
  readonly machine: string;
  /** The working directory of the turn, as efrd sent it. */
  readonly cwd: string | undefined;
  /** The phone has a ready connection to the machine now. */
  readonly connected: boolean;
  /** The header text of the time to answer (`expiryLabel`), when the question has one. */
  readonly expiry: string | undefined;
  readonly answer: AnswerState;
}

/** The buttons of an open block. */
export interface ApprovalButtons {
  readonly enabled: boolean;
  /** The answer on its way: Allow shows the braille cell, Deny dims. */
  readonly sending: PhoneDecision | undefined;
}

/** A block that still takes an answer: the gold outline. */
export interface OpenApproval {
  readonly kind: "open";
  /** The VoiceOver label of the group. */
  readonly label: string;
  readonly expiry: string | undefined;
  readonly subject: Subject;
  /** The cwd in its short form, such as `~/code/efr`. */
  readonly cwd: string | undefined;
  /** The other subjects and lines of the summary. */
  readonly details: readonly string[];
  readonly diff: readonly DiffRow[] | undefined;
  readonly reason: readonly ReasonSpan[] | undefined;
  readonly terminal: { readonly line: string; readonly note: string } | undefined;
  /** A quiet line over the buttons, such as the offline hint. */
  readonly hint: string | undefined;
  /** The sentence of the failed answer. */
  readonly error: string | undefined;
  /** None for a question that only the terminal can answer. */
  readonly buttons: ApprovalButtons | undefined;
}

/** The glyph of a closed block. */
export type ClosedGlyph = "allowed" | "denied" | "expired";

/** A block that takes no answer now: the quiet line. */
export interface ClosedApproval {
  readonly kind: "closed";
  readonly glyph: ClosedGlyph;
  /** `allowed on this phone · 14:32`. */
  readonly text: string;
  /** The command on one line, such as `$ git push`. */
  readonly subject: string;
  readonly label: string;
}

/** The model of an approval block. */
export type ApprovalBlockModel = OpenApproval | ClosedApproval;

/** The block of `approval`. */
export function approvalBlock(
  approval: ApprovalView,
  context: ApprovalBlockContext,
): ApprovalBlockModel {
  const diff =
    approval.diffPreview === undefined ? undefined : approvalDiffRows(approval.diffPreview);
  const count =
    approval.diffPreview === undefined ? undefined : diffStat(diffLines(approval.diffPreview));
  const parts = summaryParts(approval.summary, count);
  if (approval.status !== "pending") {
    return closedBlock(approval, subjectLine(parts.subject), context);
  }
  const reason = terminalReason(approval);
  const local = reason !== undefined && reason !== "always";
  const timeUp = context.expiry === TIME_UP;
  const sending = context.answer.sending;
  const buttons: ApprovalButtons | undefined = local
    ? undefined
    : { enabled: context.connected && sending === undefined && !timeUp, sending };
  let hint: string | undefined;
  if (buttons !== undefined && !context.connected && sending === undefined) {
    hint = `You can answer when ${context.machine} is connected again.`;
  }
  const failed = context.answer.failed;
  return {
    kind: "open",
    label: local ? `Approval needed, answer on ${context.machine}` : "Approval needed",
    expiry: context.expiry,
    subject: parts.subject,
    cwd: context.cwd === undefined ? undefined : shortPath(context.cwd),
    details: parts.details,
    diff: diff !== undefined && diff.length > 0 ? diff : undefined,
    reason: approval.reason === undefined ? undefined : reasonSpans(approval.reason),
    terminal:
      reason === undefined
        ? undefined
        : { line: terminalLine(reason, context.machine), note: TERMINAL_NOTES[reason] },
    hint,
    error:
      failed === undefined || local ? undefined : answerErrorText(failed.error, context.machine),
    buttons,
  };
}

function closedBlock(
  approval: ApprovalView,
  subject: string,
  context: ApprovalBlockContext,
): ClosedApproval {
  const parts: string[] = [];
  let glyph: ClosedGlyph = "expired";
  switch (approval.status) {
    case "allowed":
    case "denied": {
      glyph = approval.status;
      const verb =
        approval.decision === "allow_always"
          ? "always allowed"
          : approval.status === "allowed"
            ? "allowed"
            : "denied";
      const where = answeredWhere(approval, context);
      parts.push(where === undefined ? verb : `${verb} ${where}`);
      if (approval.closedAt !== undefined) {
        parts.push(clockTime(approval.closedAt));
      }
      break;
    }
    case "expired_away":
      parts.push("expired while you were away");
      if (approval.askedAt !== undefined && approval.closedAt !== undefined) {
        parts.push(`${clockTime(approval.askedAt)}-${clockTime(approval.closedAt)}`);
      } else if (approval.closedAt !== undefined) {
        parts.push(clockTime(approval.closedAt));
      }
      break;
    case "settled":
      parts.push("closed", "the phone did not see the answer");
      break;
    default:
      parts.push("expired", "not answered");
      if (approval.closedAt !== undefined) {
        parts.push(clockTime(approval.closedAt));
      }
      break;
  }
  return {
    kind: "closed",
    glyph,
    text: parts.join(" · "),
    subject,
    label: `${capitalized(parts.join(", "))}: ${subject}`,
  };
}

/** Where the answer came from: `on this phone`, `in a terminal on desktop`, or none. */
function answeredWhere(approval: ApprovalView, context: ApprovalBlockContext): string | undefined {
  switch (approval.answeredBy) {
    case "phone":
      return context.answer.answeredHere ? "on this phone" : "on a phone";
    case "shell":
    case "cli":
    case "proxy":
      return `in a terminal on ${context.machine}`;
    default:
      return undefined;
  }
}

function capitalized(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}
