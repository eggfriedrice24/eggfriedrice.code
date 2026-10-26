// What an approval block shows: its title, the notes under the summary, the countdown
// and the buttons, on the one approval type of the stream reducer. Pure, so each state
// has a test.
//
// The phone answers only `allow` and `deny` (R3.10 of the research): it never offers
// "always", and it gives no buttons for a question that only the user's own machine may
// answer (`local_only`); efrd refuses such an answer from a phone and keeps the question
// open. An expired question says in clear words that the call did not run, and whether
// the user was away.

import type { ApprovalDecision, Origin } from "@/lib/protocol/generated";
import type { ApprovalView } from "@/lib/stream/reducer";
import { clock } from "./format";

/** An answer that the phone can give. */
export type PhoneDecision = "allow" | "deny";

/** Why the phone cannot give an answer. */
export type AnswerRefusal = "unknown" | "not_pending" | "local_only" | "decision";

/** The answers that the phone may give to `approval`: none when it is not pending or local only. */
export function phoneDecisions(approval: ApprovalView): readonly PhoneDecision[] {
  return approval.status === "pending" && !approval.localOnly ? ["allow", "deny"] : [];
}

/**
 * Why the phone must not send `decision` for `approval`, or `undefined` when it may. The
 * mutation checks it before the call, so the card and the call read the same state.
 */
export function answerRefusal(
  approval: ApprovalView | undefined,
  decision: ApprovalDecision,
): AnswerRefusal | undefined {
  if (approval === undefined) {
    return "unknown";
  }
  if (approval.status !== "pending") {
    return "not_pending";
  }
  if (approval.localOnly) {
    return "local_only";
  }
  return decision === "allow" || decision === "deny" ? undefined : "decision";
}

/** Why only the user's own machine may answer a question. */
export type LocalReason = "privilege" | "delete" | "job";

/**
 * Why `approval` waits for an answer on the computer, or `undefined` when the phone may
 * answer. efrd sets `local_only` for a privilege exit or a command that runs as another
 * user, for a delete that full mode guards (it has a fixed limit) and for a job's
 * question.
 */
export function localReason(approval: ApprovalView): LocalReason | undefined {
  if (!approval.localOnly) {
    return undefined;
  }
  if (approval.exitKinds.includes("privilege") || approval.interactive) {
    return "privilege";
  }
  if (approval.expiresAt !== undefined || approval.exitKinds.includes("destructive")) {
    return "delete";
  }
  return "job";
}

const LOCAL_NOTES: Readonly<Record<LocalReason, string>> = {
  privilege: "It needs your password, which only the terminal can take.",
  delete: "Only your computer can approve this delete.",
  job: "It keeps running after the turn ends, so efr only starts it from the terminal.",
};

/** The colour role of the block's title. */
export type ApprovalTone = "warning" | "muted" | "success" | "error";

/** A button of the block. */
export interface ApprovalButton {
  readonly decision: PhoneDecision;
  readonly label: string;
  readonly enabled: boolean;
}

/** The block of one approval question. */
export interface ApprovalCardModel {
  readonly title: string;
  readonly tone: ApprovalTone;
  /** Lines under the summary, each a short sentence. */
  readonly notes: readonly string[];
  /** `Expires in 1:45`, for a pending question with a fixed limit. */
  readonly countdown: string | undefined;
  readonly buttons: readonly ApprovalButton[];
  /** One sentence for VoiceOver: the title, the summary and the state. */
  readonly label: string;
}

/** What the block knows besides the question. */
export interface ApprovalContext {
  /** The phone has a connection to efrd now. */
  readonly connected: boolean;
  /** The answer that the phone sent and efrd has not confirmed yet. */
  readonly sending: PhoneDecision | undefined;
  /** Why the last answer failed, in one sentence. */
  readonly error: string | undefined;
}

/** The block of `approval` at time `now` (ms since the epoch). */
export function approvalCard(
  approval: ApprovalView,
  now: number,
  context: ApprovalContext,
): ApprovalCardModel {
  switch (approval.status) {
    case "pending":
      return pendingCard(approval, now, context);
    case "allowed":
    case "denied":
      return answeredCard(approval);
    case "expired":
    case "expired_away":
      return expiredCard(approval);
    case "settled":
      return {
        title: "Closed",
        tone: "muted",
        notes: ["The question closed. The app did not get the answer."],
        countdown: undefined,
        buttons: [],
        label: `Closed: ${approval.summary}`,
      };
    default:
      return expiredCard(approval);
  }
}

function pendingCard(
  approval: ApprovalView,
  now: number,
  context: ApprovalContext,
): ApprovalCardModel {
  const notes: string[] = [];
  const secondsLeft =
    approval.expiresAt === undefined ? undefined : (approval.expiresAt - now) / 1000;
  const countdown =
    secondsLeft === undefined
      ? undefined
      : secondsLeft > 0
        ? `Expires in ${clock(secondsLeft)}`
        : "The time to answer is over";
  const local = localReason(approval);
  if (local !== undefined) {
    notes.push(LOCAL_NOTES[local]);
    return {
      title: "Answer on your computer",
      tone: "warning",
      notes,
      countdown,
      buttons: [],
      label: `Approval needed, answer on your computer: ${approval.summary}`,
    };
  }
  if (approval.always !== undefined) {
    notes.push('An "always" rule can only be set in the terminal.');
  }
  if (!context.connected) {
    notes.push("You can answer when the phone connects to your computer again.");
  }
  if (context.error !== undefined) {
    notes.push(context.error);
  }
  const open = context.connected && context.sending === undefined && (secondsLeft ?? 1) > 0;
  const buttons: ApprovalButton[] = [
    { decision: "deny", label: context.sending === "deny" ? "Denying" : "Deny", enabled: open },
    {
      decision: "allow",
      label: context.sending === "allow" ? "Allowing" : "Allow",
      enabled: open,
    },
  ];
  return {
    title: "Approval needed",
    tone: "warning",
    notes,
    countdown,
    buttons,
    label: `Approval needed: ${approval.summary}`,
  };
}

function answeredCard(approval: ApprovalView): ApprovalCardModel {
  const where = answeredWhere(approval.answeredBy);
  const allowed = approval.status === "allowed";
  const title =
    approval.decision === "allow_always" ? "Allowed always" : allowed ? "Allowed" : "Denied";
  return {
    title,
    tone: allowed ? "success" : "error",
    notes: where === undefined ? [] : [`Answered ${where}.`],
    countdown: undefined,
    buttons: [],
    label: `${title}: ${approval.summary}`,
  };
}

function expiredCard(approval: ApprovalView): ApprovalCardModel {
  const away = approval.status === "expired_away";
  const title = away ? "Expired while you were away" : "Expired";
  const notes = away
    ? ["Nobody answered while you were away. The call did not run."]
    : ["Nobody answered in time, or the turn stopped. The call did not run."];
  return {
    title,
    tone: "muted",
    notes,
    countdown: undefined,
    buttons: [],
    label: `${title}: ${approval.summary}`,
  };
}

/** Where an answer came from: `on a phone`, `in a terminal`, or `undefined`. */
export function answeredWhere(origin: Origin | undefined): string | undefined {
  switch (origin) {
    case undefined:
      return undefined;
    case "phone":
      return "on a phone";
    case "shell":
    case "cli":
    case "proxy":
      return "in a terminal";
    default:
      return undefined;
  }
}

/** The time of the next change of the block's countdown, or `undefined` when it has none. */
export function nextTick(approval: ApprovalView, now: number): number | undefined {
  if (approval.status !== "pending" || approval.expiresAt === undefined) {
    return undefined;
  }
  const left = approval.expiresAt - now;
  if (left <= 0) {
    return undefined;
  }
  const intoSecond = left % 1000;
  return now + (intoSecond === 0 ? 1000 : intoSecond);
}
