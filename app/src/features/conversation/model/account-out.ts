// A turn that failed because its account cannot serve it (AC-Out): the reason in efr's
// words, and an offer of the same model on the other accounts. The phone never switches
// by itself.
//
// - The reason is the sentence of efrd ("the ChatGPT plan reached its usage limit"),
//   then "· resets 14:20" when the reset is known. efr does not know which window, so
//   the line names none.
// - The offer shows only under the newest turn of the conversation, before the reset:
//   in the history it is old. One plain "switch to <account>" per account that serves
//   the model (efrd lists only those that are ready, or full with an earlier reset).
//   A signed-out account gets one quiet line with its login command, and no action.
//   With no other account: "no other account runs GPT-5.5", and retry stays.
// - A tap on a switch action opens the model sheet with that model selected on that
//   account. After the confirm, the switch actions become "retry".
//
// Pure TypeScript.

import type { AccountOut, LimitReached } from "@/lib/protocol/generated";
import { accountName, accountShort, loginCommand, resetText } from "./accounts";
import type { ModelChoice } from "./model-choices";
import { modelName } from "./model-names";

/** A switch action: the model sheet opens with `model` on `provider` selected. */
export interface SwitchAction {
  readonly label: string;
  readonly model: string;
  readonly provider: string;
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

function resetAt(reached: LimitReached): number | undefined {
  const at = reached.resets_at ?? undefined;
  if (at === undefined) {
    return undefined;
  }
  const parsed = Date.parse(at);
  return Number.isNaN(parsed) ? undefined : parsed;
}

/**
 * The sentence of `error` with the reset of `out`, as the turn end shows it: "resets
 * 14:20" today, "resets Mon 09:00" on another day.
 */
export function accountOutReason(out: AccountOut, error: string | undefined, now: number): string {
  const sentence = (error ?? "").trim().replace(/\.$/u, "");
  const base = sentence === "" ? "the account cannot serve this model now" : sentence;
  const at = resetAt(out.reached);
  return at === undefined ? base : `${base} · ${resetText(at, now)}`;
}

/** The view of `out` under the end of its turn. */
export function accountOutView(
  out: AccountOut,
  error: string | undefined,
  context: AccountOutContext,
): AccountOutView {
  const reason = accountOutReason(out, error, context.now);
  const reset = resetAt(out.reached);
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
  const switches = offerable.map((other) => ({
    label: `switch to ${accountName(other.provider, other.label)}`,
    model: out.model,
    provider: other.provider,
  }));
  const notes = others
    .filter((other) => other.state === "signed_out")
    .map(
      (other) =>
        `${accountShort(other.provider, other.label)} is signed out: ${loginCommand(other.provider)}`,
    );
  if (others.length === 0) {
    notes.push(`no other account runs ${modelName(out.model)}`);
  }
  return { reason, switches, notes, retry: switches.length === 0 };
}
