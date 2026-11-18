// The question of the model on the phone (board S-Ask): the model asks one question with
// 2 to 4 options, and the phone answers with an option, with a typed text, or with a
// skip. An answer is information for the model, never a permission: a later approval
// still asks. Pure, so each state has a test.
//
// - The strip is the slot of the approval strip: the yolk dot, the question on one line,
//   then Answer. A tap opens the native action sheet. It never opens by itself.
// - The action sheet (ActionSheetIOS): the question as the title, one line per option
//   ("label: description") as the message, so the full text is there when iOS cuts a
//   button, then the labels in the model's order, "Type an answer", "Skip" and Cancel.
// - "Type an answer" puts the composer in the answer state: Send answers the question,
//   and the fixed hint stands under the field.
// - The row of the `ask_user` call says how the question ended.

import type { ActionSheet } from "@/components/ui/action-sheet";
import type { QuestionReply } from "@/lib/protocol/generated";
import type { UserQuestionView } from "@/lib/stream/reducer";
import { utf8Length } from "@/lib/stream/utf8";
import { withoutWakeMark } from "./approval-block-text";
import { metaOf, type ApprovalMeta, type ApprovalQueue } from "./approval-strip";
import { TERMINAL_ORIGINS } from "./quarantine";

/** The most bytes of a typed answer (efr-protocol's `QuestionReply::MAX_TEXT_BYTES`). */
export const MAX_ANSWER_BYTES = 1024;

/** The fixed hint under the field while the user types an answer. */
export const ANSWER_HINT = "Do not type a password or a token here.";

/** The placeholder of the empty field while the user types an answer. */
export const ANSWER_PLACEHOLDER = "Your answer";

/** The VoiceOver label of Send while the user types an answer. */
export const SEND_ANSWER_LABEL = "Send the answer";

/**
 * The question on one line, without the mark of a job notice turn that efrd puts in
 * front of it: the strip, the row and the answer row show it so.
 */
export function questionLine(question: UserQuestionView): string {
  const text = question.question;
  return (withoutWakeMark(text) ?? text).replace(/\s+/g, " ").trim();
}

/** What the strip of a question knows about the phone's answer. */
export interface QuestionStripContext {
  /** The phone has a ready connection to the machine now. */
  readonly connected: boolean;
  /** True while efrd has not taken the answer of the phone yet. */
  readonly sending: boolean;
  /** True when the last answer of the phone did not arrive. */
  readonly failed: boolean;
}

/** The strip of the question that waits first. */
export interface QuestionStripModel {
  readonly questionId: string;
  readonly callId: string;
  /** The question on one line. */
  readonly line: string;
  /** The VoiceOver label of the strip. */
  readonly label: string;
  readonly meta: readonly ApprovalMeta[];
  readonly sending: boolean;
  /** False while the phone cannot answer: offline, or an answer in flight. */
  readonly enabled: boolean;
}

/**
 * True while the phone can answer a question: it is online, and no answer of that
 * question is in flight. The strip and the row of the call read the same rule.
 */
export function canAnswer(connected: boolean, sending: boolean): boolean {
  return connected && !sending;
}

/** The strip of `question` at `queue`, or `undefined` when it does not wait. */
export function questionStrip(
  question: UserQuestionView,
  context: QuestionStripContext,
  queue: ApprovalQueue,
): QuestionStripModel | undefined {
  if (question.answer.kind !== "pending") {
    return undefined;
  }
  const line = questionLine(question);
  return {
    questionId: question.questionId,
    callId: question.callId,
    line,
    label: `Question: ${line}`,
    meta: metaOf(context.failed, undefined, queue, context.connected),
    sending: context.sending,
    enabled: canAnswer(context.connected, context.sending),
  };
}

/** The ids of the actions of the sheet that are not an option. */
const TYPE_ACTION = "type";
const SKIP_ACTION = "skip";

/** The native action sheet of `question`. */
export function questionSheet(question: UserQuestionView): ActionSheet {
  const message = question.options
    .map((option) =>
      option.description === undefined || option.description === null
        ? option.label
        : `${option.label}: ${option.description}`,
    )
    .join("\n");
  return {
    title: question.question,
    message,
    actions: [
      ...question.options.map((option, index) => ({
        id: `option:${String(index)}`,
        label: option.label,
      })),
      { id: TYPE_ACTION, label: "Type an answer" },
      { id: SKIP_ACTION, label: "Skip" },
    ],
  };
}

/** What a pick in the sheet does: an answer to send, or the answer state of the composer. */
export type QuestionChoice =
  | { readonly kind: "reply"; readonly reply: QuestionReply }
  | { readonly kind: "type" };

/** The choice of the action `id` of `questionSheet`, or none for Cancel. */
export function questionChoice(id: string | undefined): QuestionChoice | undefined {
  if (id === TYPE_ACTION) {
    return { kind: "type" };
  }
  if (id === SKIP_ACTION) {
    return { kind: "reply", reply: { kind: "skipped" } };
  }
  const index = id === undefined ? Number.NaN : Number(/^option:(\d+)$/.exec(id)?.[1]);
  return Number.isInteger(index) ? { kind: "reply", reply: { kind: "option", index } } : undefined;
}

/**
 * The typed answer as efrd takes it, or `undefined` when it cannot go: line ends and other
 * control characters become spaces, the ends are trimmed, and the rest must be neither
 * empty nor longer than MAX_ANSWER_BYTES.
 */
export function answerText(typed: string): string | undefined {
  // oxlint-disable-next-line no-control-regex -- control characters are what this removes
  const text = typed.replace(/[\u0000-\u001f\u007f-\u009f]+/g, " ").trim();
  return text.length === 0 || utf8Length(text) > MAX_ANSWER_BYTES ? undefined : text;
}

/** The row of the `ask_user` call: the question, and how it ended. */
export interface QuestionRow {
  readonly question: string;
  /** `waiting for you`, `answered · Postgres 16`, `skipped`, `nobody answered`. */
  readonly state: string;
  /** True while it waits: the yolk dot, and the state in accent_primary. */
  readonly waiting: boolean;
}

/** The row of `question`. `sending` is true while the phone's answer is in flight. */
export function questionRow(
  question: UserQuestionView,
  machineName: string,
  sending: boolean,
): QuestionRow {
  const row = (state: string, waiting = false): QuestionRow => ({
    question: questionLine(question),
    state,
    waiting,
  });
  const answer = question.answer;
  const origin = question.origin;
  const answered =
    origin !== undefined && TERMINAL_ORIGINS.has(origin)
      ? `answered on ${machineName}`
      : "answered";
  switch (answer.kind) {
    case "pending":
      return sending ? row("sending…", true) : row("waiting for you", true);
    case "option":
      return row(`${answered} · ${answer.label}`);
    case "text":
      return row(`${answered} · "${answer.text.replace(/\s+/g, " ")}"`);
    case "skipped":
      return row("skipped");
    case "unanswered":
      return row("nobody answered");
    default:
      return row("not answered · the turn was interrupted");
  }
}
