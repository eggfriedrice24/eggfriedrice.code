// What an approval card shows: its title, the note under the summary, the countdown and
// the buttons. Pure, so each state has a test.
//
// The phone answers only `allow` and `deny` (R3.10 of the research): it never offers
// "always", and it gives no buttons for a question that only the user's own machine may
// answer (`local_only`); efrd refuses such an answer from a phone and keeps the question
// open. An expired question says in clear words that the call did not run, and whether
// the user was away.

import type { ApprovalView } from "./details";
import { clock } from "./format";

/** An answer that the phone can give. */
export type PhoneDecision = "allow" | "deny";

/** A button of the card. */
export interface ApprovalButton {
  readonly decision: PhoneDecision;
  readonly label: string;
  readonly enabled: boolean;
}

/** The colour role of the card's title. */
export type ApprovalTone = "warning" | "muted" | "success" | "error";

/** The card of one approval question. */
export interface ApprovalCardModel {
  readonly title: string;
  readonly tone: ApprovalTone;
  /** Lines under the summary, each a short sentence. */
  readonly notes: readonly string[];
  /** `Expires in 1:45`, for a pending question with a fixed limit. */
  readonly countdown?: string;
  readonly buttons: readonly ApprovalButton[];
  /** One sentence for VoiceOver: the title, the summary and the state. */
  readonly label: string;
}

/** What the card knows besides the question. */
export interface ApprovalContext {
  /** The phone has a connection to efrd now. */
  readonly connected: boolean;
  /** The answer that the phone sent and efrd has not confirmed yet. */
  readonly sending?: PhoneDecision;
  /** Why the last answer failed, in one sentence. */
  readonly error?: string;
}

/** The card of `approval` at time `now` (ms since the epoch). */
export function approvalCard(
  approval: ApprovalView,
  now: number,
  context: ApprovalContext,
): ApprovalCardModel {
  if (approval.state === "resolved") {
    return resolvedCard(approval);
  }
  if (approval.state === "expired") {
    return expiredCard(approval);
  }
  return pendingCard(approval, now, context);
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
  if (approval.localOnly) {
    notes.push("Only your computer can answer this question. The turn waits for the answer.");
    return {
      title: "Answer on your computer",
      tone: "warning",
      notes,
      ...(countdown === undefined ? {} : { countdown }),
      buttons: [],
      label: `Answer on your computer: ${approval.summary}`,
    };
  }
  if (approval.interactive) {
    notes.push(
      "The command can ask for input, such as a password. You cannot type it on the phone.",
    );
  }
  if (!context.connected) {
    notes.push("You can answer when the phone connects to your computer again.");
  }
  if (context.error !== undefined) {
    notes.push(context.error);
  }
  const open = context.connected && context.sending === undefined && (secondsLeft ?? 1) > 0;
  const buttons: ApprovalButton[] = [
    {
      decision: "deny",
      label: context.sending === "deny" ? "Denying" : "Deny",
      enabled: open,
    },
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
    ...(countdown === undefined ? {} : { countdown }),
    buttons,
    label: `Approval needed: ${approval.summary}`,
  };
}

function resolvedCard(approval: ApprovalView): ApprovalCardModel {
  const where = answeredWhere(approval.answeredBy);
  const allowed = approval.decision === "allow" || approval.decision === "allow_always";
  const title =
    approval.decision === "allow_always"
      ? "Allowed always"
      : allowed
        ? "Allowed"
        : approval.decision === "deny"
          ? "Denied"
          : "Answered";
  const notes = where === undefined ? [] : [`Answered ${where}.`];
  return {
    title,
    tone: allowed ? "success" : "error",
    notes,
    buttons: [],
    label: `${title}: ${approval.summary}`,
  };
}

function expiredCard(approval: ApprovalView): ApprovalCardModel {
  const title = approval.away ? "Expired while you were away" : "Expired";
  const notes = approval.away
    ? ["Nobody answered while you were away. The call did not run."]
    : ["Nobody answered in time, or the turn stopped. The call did not run."];
  return { title, tone: "muted", notes, buttons: [], label: `${title}: ${approval.summary}` };
}

function answeredWhere(origin: string | undefined): string | undefined {
  switch (origin) {
    case undefined:
    case "":
      return undefined;
    case "phone":
      return "on a phone";
    case "shell":
    case "cli":
    case "proxy":
      return "on your computer";
    default:
      return `by ${origin}`;
  }
}

/** The time of the next change of the card's countdown, or `undefined` when it has none. */
export function nextTick(approval: ApprovalView, now: number): number | undefined {
  if (approval.state !== "pending" || approval.expiresAt === undefined) {
    return undefined;
  }
  const left = approval.expiresAt - now;
  if (left <= 0) {
    return undefined;
  }
  const intoSecond = left % 1000;
  return now + (intoSecond === 0 ? 1000 : intoSecond);
}
