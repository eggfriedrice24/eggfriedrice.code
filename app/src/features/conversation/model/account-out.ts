// A turn that failed because its account cannot serve it (AC-Out): the reason in efr's
// words, and an offer of the same model on the other accounts. The phone never switches
// by itself.
//
// - The reason is built from the account and the kind of the limit, in the words of the
//   terminal ("the ChatGPT plan reached its usage limit", "b has no credit left"), then
//   "· resets 14:20" when the reset is known. The message of the error is not used: it
//   names no account and can carry the provider's own text. efr does not know which
//   window, so the line names none. Credit has no reset, and "too many requests" shows
//   none: its reset is the end of a wait of some seconds (errata 5).
// - The offer shows only under the newest turn of the conversation, before the reset:
//   in the history it is old. One plain "switch to <account>" per account that serves
//   the model now. A full account whose reset comes first is no ready one: its action
//   says when it runs again, in the words of the terminal (TM-Accounts 17), such as
//   "b runs again 14:20", and the tap still opens the sheet on it.
//   A signed-out account gets one quiet line with its login command, and no action.
//   When each other account of the model is full until after the stopped one: "no other
//   account has room for GPT-6.1 Sol before 06:02", and retry stays. With no other
//   account: "no other account runs GPT-5.5", and retry stays.
// - A tap on a switch action opens the model sheet with that model selected on that
//   account. After the confirm, the switch actions become "retry".
//
// Pure TypeScript.

import type { AccountOut, LimitKind, LimitReached } from "@/lib/protocol/generated";
import {
  accountName,
  accountShort,
  accountSubject,
  isoMs,
  loginCommand,
  resetClock,
  resetText,
} from "./accounts";
import type { ModelChoice } from "./model-choices";
import { modelName } from "./model-names";

/** A switch action: the model sheet opens with `model` on `provider` selected. */
export interface SwitchAction {
  readonly label: string;
  readonly model: string;
  readonly provider: string;
  /** What VoiceOver reads when the label alone names no action. */
  readonly accessibilityLabel?: string;
}

/** What the end of the turn shows under its line. */
export interface AccountOutView {
  /** The reason, in place of the plain error. */
  readonly reason: string;
  /** The switch actions; empty when there is no offer now. */
  readonly switches: readonly SwitchAction[];
  /** The quiet lines of the offer: the signed-out accounts, or "no other account". */
  readonly notes: readonly string[];
  /** True when "retry" shows. */
  readonly retry: boolean;
}

/** What the view needs to know of now. */
export interface AccountOutContext {
  /** The turn is the newest of the conversation. */
  readonly newest: boolean;
  readonly now: number;
  /** The model, account and effort that the composer chose for the next message. */
  readonly choice: ModelChoice | undefined;
}

/**
 * The reset that the stop of `reached` names, as the terminal's `shown_reset`. Credit has
 * no reset, and the reset of "too many requests" is only the end of a wait of some
 * seconds, which TM-Accounts 5 and errata 5 leave out: "efr stopped trying" is enough.
 * The same reset decides when the offer is old.
 */
function shownReset(reached: LimitReached): number | undefined {
  if (reached.kind === "credit" || reached.kind === "too_many_requests") {
    return undefined;
  }
  return isoMs(reached.resets_at);
}

/** What the account did, after its name, as the terminal says it. */
const LIMIT_SENTENCES: Readonly<Record<LimitKind, string>> = {
  usage: "reached its usage limit",
  requests: "reached its rate limit",
  tokens: "reached its rate limit",
  input_tokens: "reached its rate limit",
  output_tokens: "reached its rate limit",
  too_many_requests: 'got "too many requests", so efr stopped trying',
  credit: "has no credit left",
  spend_limit: "reached its spend limit",
  unknown: "reached a limit",
};

/**
 * The reason of `out` as the turn end shows it: the account and what it did, with the
 * reset, "resets 14:20" today, "resets Mon 09:00" on another day.
 */
export function accountOutReason(out: AccountOut, now: number): string {
  const subject = accountSubject(out.provider, out.label);
  const base = `${subject} ${LIMIT_SENTENCES[out.reached.kind]}`;
  const at = shownReset(out.reached);
  return at === undefined ? base : `${base} · ${resetText(at, now)}`;
}

/** The view of `out` under the end of its turn. */
export function accountOutView(out: AccountOut, context: AccountOutContext): AccountOutView {
  const reason = accountOutReason(out, context.now);
  const reset = shownReset(out.reached);
  const others = out.others ?? [];
  const offerable = others.filter((other) => other.state === "ready" || other.state === "full");
  const switched =
    context.choice !== undefined &&
    context.choice.model === out.model &&
    context.choice.provider !== undefined &&
    context.choice.provider !== out.provider;
  const stale = !context.newest || (reset !== undefined && reset <= context.now);
  if (stale) {
    // The history keeps the reason; the offer is old. After the reset the account can
    // serve again, so the newest turn offers retry.
    return { reason, switches: [], notes: [], retry: context.newest };
  }
  if (switched) {
    return { reason, switches: [], notes: [], retry: true };
  }
  const switches = offerable.map((other): SwitchAction => {
    const name = accountName(other.provider, other.label);
    const free = other.state === "full" ? isoMs(other.resets_at) : undefined;
    if (free === undefined) {
      return { label: `switch to ${name}`, model: out.model, provider: other.provider };
    }
    // NOTE: a full account is no ready one: it says when it runs again, as the terminal
    // does (TM-Accounts 17), and the tap still opens the sheet on it.
    const when = resetClock(free, context.now);
    return {
      label: `${accountShort(other.provider, other.label)} runs again ${when}`,
      model: out.model,
      provider: other.provider,
      accessibilityLabel: `Switch to ${name}, it runs again at ${when}`,
    };
  });
  const notes: string[] = [];
  const room = out.others_full ?? undefined;
  if (offerable.length === 0 && room !== undefined) {
    notes.push(roomNote(out.model, isoMs(room.resets_at), context.now));
  }
  for (const other of others) {
    if (other.state === "signed_out") {
      notes.push(
        `${accountShort(other.provider, other.label)} is signed out: ${loginCommand(other.provider)}`,
      );
    }
  }
  if (others.length === 0 && room === undefined) {
    notes.push(`no other account runs ${modelName(out.model)}`);
  }
  return { reason, switches, notes, retry: switches.length === 0 };
}

/**
 * The quiet line when each other account of `model` is full until after the stopped one
 * (`others_full`): they run the model, but none has room before the soonest of their
 * resets, as the terminal says it.
 */
function roomNote(model: string, at: number | undefined, now: number): string {
  const base = `no other account has room for ${modelName(model)}`;
  return at === undefined ? base : `${base} before ${resetClock(at, now)}`;
}
