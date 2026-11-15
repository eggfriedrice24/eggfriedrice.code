// The one rule of a new prompt of a composer, for a send, a queue and a retry (decision
// D8: efrd has no retry of a turn, so the prompt goes again as a new one):
//
// - the prompt carries `promptChoice`: the choice of the model sheet, else on a new
//   thread the favourite of the last plan;
// - it carries auto when the mode of the composer is auto and the Face ID unlock of the
//   app is on at the moment of the call (`effectiveMode`);
// - a prompt that names an account makes it the last plan of the machine.
//
// Pure TypeScript on the stores of the app.

import type { ModelsListResult } from "@/lib/protocol/generated";
import { composerDraftKey, outboxInput, type ComposerTarget } from "./composer-state";
import { effectiveMode, type ModelChoices } from "./model-choices";
import { promptChoice } from "./model-sheet";
import type { Outbox, OutboxEntry } from "./outbox";

/** What a new prompt needs from the app. */
export interface SendPromptDeps {
  readonly outbox: Outbox;
  readonly modelChoices: ModelChoices;
  /** True when the Face ID unlock of the app is on at the moment of the call. */
  readonly lockOn: boolean;
}

/** Puts `text` in the outbox as a new prompt of `target`, by the rule of the module. */
export function sendPrompt(
  deps: SendPromptDeps,
  list: ModelsListResult | undefined,
  target: ComposerTarget,
  text: string,
): OutboxEntry {
  const { outbox, modelChoices } = deps;
  const key = composerDraftKey(target);
  const state = modelChoices.store.get();
  const choice = promptChoice({
    list,
    machine: target.machine,
    isBlankRoot: target.isBlankRoot,
    choice: state.choices.get(key),
    favourites: state.favourites,
    plan: state.plans.get(target.machine),
  });
  const auto = effectiveMode(modelChoices.mode(key), deps.lockOn) === "auto";
  const entry = outbox.enqueue(outboxInput(outbox.store.get(), target, text, choice, auto));
  if (choice?.provider !== undefined) {
    modelChoices.setPlan(target.machine, choice.provider);
  }
  return entry;
}
