// The approval strip of the composer and the approval sheet (boards C-Waiting,
// S-Approvals, AP-Sheets and R-Approval, round 8 "minimal"), from the model of an
// approval block (approval-block.ts). Pure, so each state has a test.
//
// - The strip is one line above the status line: the yolk dot, the command on one line
//   (cut at its end), then small text only when it matters ("1 of 3", the seconds under
//   a minute, "offline"), then Deny and Allow. A question that only the terminal can
//   answer shows "answer on <machine>" in place of the buttons.
// - The sheet has no title, no labels and no boxes: the full command (or the edit line
//   and the diff), one quiet line (where it runs and why it asks), the small text, then
//   Deny and Allow. A question that closed while the sheet is open dims and says how it
//   ended and how many more wait. Close and, when more wait, Next are items of the native
//   header of the sheet (`nextQuestion`).
// - Neither opens by itself: a tap on the strip or on the waiting tool card opens the
//   sheet.

import type { ApprovalView, QuarantineQuestionView, ToolCallView } from "@/lib/stream/reducer";
import {
  approvalBlock,
  terminalReason,
  type ApprovalBlockContext,
  type ApprovalButtons,
  type DiffRow,
  type TerminalReason,
} from "./approval-block";
import { subjectLine, summaryParts, TIME_UP, type Subject } from "./approval-block-text";
import { grantList, reviewerRow } from "./exit-words";

/** The place of a question among the questions that wait. */
export interface ApprovalQueue {
  /** Its place, from 0. */
  readonly index: number;
  /** How many wait in the conversation. */
  readonly count: number;
}

/** One small text of an open question. */
export interface ApprovalMeta {
  readonly key: "queue" | "time" | "offline" | "failed";
  readonly text: string;
  readonly tone: "text_subtle" | "accent_primary" | "state_error";
}

/** The strip of the question that waits first. */
export interface ApprovalStripModel {
  readonly callId: string;
  readonly turnId: string;
  /** The command on one line, such as `git push -u origin main`. */
  readonly line: string;
  /** The VoiceOver label of the strip. */
  readonly label: string;
  readonly meta: readonly ApprovalMeta[];
  /** `answer on laptop`, in place of the buttons. */
  readonly terminal: string | undefined;
  readonly buttons: ApprovalButtons | undefined;
}

/**
 * The seconds left that the strip and the sheet show: only under a minute (`42s`), and
 * `time is up` at the end. `expiry` is the header text of `expiryLabel`.
 */
export function shortTimeLeft(expiry: string | undefined): string | undefined {
  if (expiry === undefined) {
    return undefined;
  }
  if (expiry === TIME_UP) {
    return TIME_UP;
  }
  const seconds = /^expires in (\d+s)$/.exec(expiry);
  return seconds?.[1];
}

/** The small texts of an open question: the time left, the place, offline, a failed answer. */
function metaOf(
  failed: boolean,
  expiry: string | undefined,
  queue: ApprovalQueue,
  connected: boolean,
): ApprovalMeta[] {
  const meta: ApprovalMeta[] = [];
  if (failed) {
    meta.push({ key: "failed", text: "not sent", tone: "state_error" });
  }
  const time = shortTimeLeft(expiry);
  if (time !== undefined) {
    meta.push({ key: "time", text: time, tone: "accent_primary" });
  }
  if (queue.count > 1) {
    meta.push({
      key: "queue",
      text: `${String(queue.index + 1)} of ${String(queue.count)}`,
      tone: "text_subtle",
    });
  }
  if (!connected) {
    meta.push({ key: "offline", text: "offline", tone: "text_subtle" });
  }
  return meta;
}

/** The one-line form of a subject for the strip: the command without its `$`. */
function stripLine(subject: Subject): string {
  const line = subject.kind === "command" ? subject.line : subjectLine(subject);
  return line.replace(/\s*\n\s*/g, " ").trim();
}

/**
 * The strip of `approval`, the question at `queue.index` of the questions that wait, or
 * `undefined` when it does not wait.
 */
export function approvalStrip(
  approval: ApprovalView,
  context: ApprovalBlockContext,
  queue: ApprovalQueue,
): ApprovalStripModel | undefined {
  const model = approvalBlock(approval, context);
  if (model.kind !== "open") {
    return undefined;
  }
  const line = stripLine(model.subject);
  const local = model.buttons === undefined;
  return {
    callId: approval.callId,
    turnId: approval.turnId,
    line,
    label: `Approval needed: ${line}`,
    meta: metaOf(model.error !== undefined, model.expiry, queue, context.connected),
    terminal: local ? model.terminal?.line : undefined,
    buttons: model.buttons,
  };
}

/**
 * The tool card of a question whose tool call the app does not hold, such as one whose
 * call started before the window of the snapshot: the call as the question names it.
 */
export function approvalCall(approval: ApprovalView): ToolCallView {
  const subject = summaryParts(approval.summary, undefined).subject;
  let tool = "";
  let summary = subject.kind === "text" ? subject.text : subjectLine(subject);
  if (subject.kind === "command") {
    tool = "shell";
    summary = subject.line;
  }
  return {
    callId: approval.callId,
    tool,
    summary,
    tail: "",
    bytes: 0,
    done: false,
    isError: false,
    exitCode: undefined,
    output: undefined,
    refusal: undefined,
    diff: undefined,
    leftRunning: false,
    startedAt: approval.askedAt,
    endedAt: undefined,
  };
}

/** Why the terminal must answer, as the quiet line of the sheet says it. */
const SHEET_TERMINAL: Readonly<Record<TerminalReason, string>> = {
  privilege: "needs your password, which only the terminal can take",
  delete: "full mode guards this delete, so only the terminal can approve it",
  job: "keeps running after the turn ends, so efr only starts it from the terminal",
  always: 'answering can add an "always" rule, and rules are only set in the terminal',
};

/** The sheet of one question. */
export interface ApprovalSheetModel {
  readonly callId: string;
  readonly subject: Subject;
  readonly diff: readonly DiffRow[] | undefined;
  /** `in ~/code/efr · pushes to a remote`: where it runs and why it asks. */
  readonly quiet: string | undefined;
  /**
   * The lines of an exit in efr's words, each only when it applies: what an allow gives
   * the call, a run with full rights, what the reviewer thought, and efr's own facts.
   */
  readonly notes: readonly string[];
  /** The question still takes an answer. */
  readonly open:
    | {
        readonly meta: readonly ApprovalMeta[];
        readonly buttons: ApprovalButtons | undefined;
        /** `answer on laptop`, in place of the buttons. */
        readonly terminal: string | undefined;
        /** The sentence of a failed answer. */
        readonly error: string | undefined;
      }
    | undefined;
  /**
   * The question closed while the sheet is open: the content dims. Next of the native
   * header shows the next question that waits (`nextQuestion`).
   */
  readonly closed:
    | {
        /** `expired · 2 more waiting`. */
        readonly parts: readonly string[];
      }
    | undefined;
}

/** The words of a run outside the sandbox, as efr's terminal has them. */
const FULL_RIGHTS = "runs with your full rights: files, secrets, network";

/** The lines of the exit of `approval` (`ExitInfo`), in efr's words. */
function exitNotes(approval: ApprovalView): string[] {
  const notes: string[] = [];
  if (approval.grants.length > 0) {
    notes.push(`an allow gives the call: ${grantList(approval.grants)}`);
  }
  if (approval.launch?.kind === "unsandboxed") {
    notes.push(FULL_RIGHTS);
  }
  const judged = approval.judged;
  if (judged !== undefined) {
    notes.push(reviewerRow(judged.verdict, judged.rationale));
  }
  notes.push(...approval.facts);
  return notes;
}

/** The sentence without its last full stop, for one part of a quiet line. */
function clause(text: string): string {
  return text.trim().replace(/\.$/, "");
}

/** A question that the sheet can show: an approval or a quarantine question. */
export interface SheetQuestion {
  readonly callId: string;
  /** True while the question waits for its answer. */
  readonly open: boolean;
}

/**
 * The question that Next of the sheet header shows: after `current` closed, the oldest
 * other question of `waiting`, approvals and quarantine questions alike (the sheet shows
 * both). None while `current` waits for its answer, and none for a question that the
 * stream does not hold.
 */
export function nextQuestion(
  current: SheetQuestion | undefined,
  waiting: readonly SheetQuestion[],
): string | undefined {
  if (current === undefined || current.open) {
    return undefined;
  }
  return waiting.find((one) => one.callId !== current.callId)?.callId;
}

/** The sheet question of an approval. */
export function approvalQuestion(approval: ApprovalView): SheetQuestion {
  return { callId: approval.callId, open: approval.status === "pending" };
}

/** The sheet question of a quarantine question. */
export function quarantineQuestion(question: QuarantineQuestionView): SheetQuestion {
  return { callId: question.callId, open: question.answer === "pending" };
}

/**
 * The sheet of `approval`. `waiting` are the questions of the conversation that wait now,
 * oldest first; `approval` may be one of them.
 */
export function approvalSheet(
  approval: ApprovalView,
  context: ApprovalBlockContext,
  waiting: readonly ApprovalView[],
): ApprovalSheetModel {
  // The layout of the question comes from its open form, also after it closed.
  const layout = approvalBlock({ ...approval, status: "pending" }, context);
  const model = approvalBlock(approval, context);
  const quiet: string[] = [];
  if (layout.kind === "open") {
    if (layout.cwd !== undefined) {
      quiet.push(`in ${layout.cwd}`);
    }
    const reason = terminalReason(approval);
    if (reason !== undefined) {
      quiet.push(SHEET_TERMINAL[reason]);
    } else if (layout.reason !== undefined) {
      quiet.push(clause(layout.reason.map((span) => span.text).join("")));
    }
    quiet.push(...layout.details);
  }
  const subject: Subject =
    layout.kind === "open" ? layout.subject : { kind: "text", text: approval.summary };
  const diff = layout.kind === "open" ? layout.diff : undefined;
  const others = waiting.filter((one) => one.callId !== approval.callId);
  const index = waiting.findIndex((one) => one.callId === approval.callId);
  const sheet = {
    callId: approval.callId,
    subject,
    diff,
    quiet: quiet.length === 0 ? undefined : quiet.join(" · "),
    notes: exitNotes(approval),
  };
  if (model.kind === "open") {
    return {
      ...sheet,
      open: {
        meta: metaOf(
          false,
          model.expiry,
          { index: Math.max(0, index), count: waiting.length },
          context.connected,
        ),
        buttons: model.buttons,
        terminal: model.buttons === undefined ? model.terminal?.line : undefined,
        error: model.error,
      },
      closed: undefined,
    };
  }
  const parts = [model.text.split(" · ")[0] ?? model.text];
  if (others.length > 0) {
    parts.push(`${String(others.length)} more waiting`);
  }
  return { ...sheet, open: undefined, closed: { parts } };
}
