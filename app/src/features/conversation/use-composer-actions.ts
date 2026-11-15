// The actions of the composer (plan section 2.6):
//
// - `send` and `queue` put the prompt in the outbox, which sends it with `prompt.send`.
//   The pending row shows at once, and a dropped socket or a killed app loses nothing. A
//   prompt while a turn runs waits in the queue of efrd, so `queue` is the same path.
//   The draft goes at once: the outbox holds the text now.
// - `steer` is the `turn.steer` mutation with `if_late: queue`, in the scope of the
//   conversation, so a steer that comes after the end of the turn becomes a queued
//   prompt. The draft stays until efrd took the steer, so a failed steer loses no text.
// - `stop` is the `turn.interrupt` mutation, with no scope: it never waits behind a
//   stuck call.
//
// Each write gets a new command id per tap.
//
// A prompt carries the choice of the model sheet. A prompt of a new thread without a
// choice carries the favourite of the last plan (`freshChoice`). A prompt that names an
// account makes it the last plan of the machine. A prompt in auto carries `auto`: the
// composer sends it only after the check of the owner passed (model/owner-check.ts).

import { useMutation, useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";

import type { MachineCaller } from "@/lib/query/machine-caller";
import type { PreferencesStore } from "@/features/settings/model/preferences";
import {
  interruptMutationOptions,
  modelsQueryOptions,
  steerMutationOptions,
  type InterruptInput,
  type SteerInput,
} from "./conversation-queries";
import {
  composerDraftKey,
  conversationIdOf,
  outboxInput,
  type ComposerTarget,
} from "./model/composer-state";
import type { Drafts } from "./model/drafts";
import { favouriteOf, type ModelChoices } from "./model/model-choices";
import { freshChoice } from "./model/model-sheet";
import type { Outbox, OutboxEntry } from "./model/outbox";
import type { CheckOwner } from "./model/owner-check";

/** What the composer needs from the app. The root layout makes each of them once. */
export interface ComposerDeps {
  /** The runtime of the machines, for the mutations. */
  readonly caller: MachineCaller;
  readonly outbox: Outbox;
  readonly drafts: Drafts;
  readonly preferences: PreferencesStore;
  /** The model, account and effort that the sheet chose per composer; a send carries it. */
  readonly modelChoices: ModelChoices;
  /** A new command id, such as `newCommandId` of src/lib/native/database.ts. */
  readonly newCommandId: () => string;
  /** Checks the owner of the iPhone before a message in auto (native/owner-check.ts). */
  readonly checkOwner: CheckOwner;
  /** True when the iPhone has a passcode: without one, Auto cannot be picked. */
  readonly hasPasscode: () => Promise<boolean>;
}

/** The actions of one composer. */
export interface ComposerActions {
  /** Sends `text` as a prompt through the outbox and clears the draft. */
  send(text: string, auto?: boolean): OutboxEntry;
  /** Queues `text` behind the running turn through the outbox and clears the draft. */
  queue(text: string, auto?: boolean): OutboxEntry;
  /** Steers the running turn with `text`. The draft goes when efrd took it. */
  steer(text: string): void;
  /** Stops the running turn. */
  stop(): void;
  /** True while a steer waits for efrd. */
  readonly steering: boolean;
  /** True while a stop waits for efrd. */
  readonly stopping: boolean;
}

/**
 * The actions of the composer of `target`. `runningTurnId` is the running turn as the
 * screen sees it: efrd steers and stops no other turn.
 */
export function useComposerActions(
  deps: ComposerDeps,
  target: ComposerTarget,
  runningTurnId: string | undefined,
): ComposerActions {
  const { caller, outbox, drafts, newCommandId } = deps;
  const key = composerDraftKey(target);
  const conversationId = useSelector(outbox.store, (snapshot) =>
    conversationIdOf(snapshot, target),
  );
  const ref = { machine: target.machine, id: conversationId };
  const steerMutation = useMutation({
    ...steerMutationOptions(caller, ref),
    onSuccess: (_result, input) => {
      // Text that the user typed after the steer stays.
      if (drafts.text(key) === input.text) {
        drafts.clear(key);
      }
    },
  });
  const stopMutation = useMutation(interruptMutationOptions(caller, ref));
  const list = useQuery(modelsQueryOptions(caller, target.machine)).data;

  const enqueue = (text: string, auto = false): OutboxEntry => {
    const choices = deps.modelChoices;
    let choice = choices.choice(key);
    if (choice === undefined && target.isBlankRoot) {
      const { favourites, plans } = choices.store.get();
      choice = freshChoice(list, plans.get(target.machine), (provider) =>
        favouriteOf(favourites, target.machine, provider),
      );
    }
    const entry = outbox.enqueue(outboxInput(outbox.store.get(), target, text, choice, auto));
    if (choice?.provider !== undefined) {
      choices.setPlan(target.machine, choice.provider);
    }
    drafts.clear(key);
    return entry;
  };

  return {
    send: enqueue,
    queue: enqueue,
    steer(text: string): void {
      const input: { -readonly [K in keyof SteerInput]: SteerInput[K] } = {
        text,
        commandId: newCommandId(),
      };
      if (runningTurnId !== undefined) {
        input.turnId = runningTurnId;
      }
      steerMutation.mutate(input);
    },
    stop(): void {
      const input: { -readonly [K in keyof InterruptInput]: InterruptInput[K] } = {
        commandId: newCommandId(),
      };
      if (runningTurnId !== undefined) {
        input.turnId = runningTurnId;
      }
      stopMutation.mutate(input);
    },
    steering: steerMutation.isPending,
    stopping: stopMutation.isPending,
  };
}
