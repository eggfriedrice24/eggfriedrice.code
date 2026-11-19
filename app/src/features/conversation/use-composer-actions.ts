// The actions of the composer (plan section 2.6):
//
// - `send` and `queue` put the prompt in the outbox, which sends it with `prompt.send`.
//   The pending row shows at once, and a dropped socket or a killed app loses nothing. A
//   prompt while a turn runs waits in the queue of efrd, so `queue` is the same path.
//   The files of the draft go with the prompt; the drain uploads them first.
//   The draft goes at once: the outbox holds the text now.
// - `steer` is the `turn.steer` mutation with `if_late: queue`, in the scope of the
//   conversation, so a steer that comes after the end of the turn becomes a queued
//   prompt. The draft stays until efrd took the steer, so a failed steer loses no text.
// - `stop` is the `turn.interrupt` mutation, with no scope: it never waits behind a
//   stuck call.
//
// Each write gets a new command id per tap.
//
// A prompt follows the one rule of model/send-prompt.ts, as a retry of the transcript
// does: the choice of the model sheet, else on a new thread the favourite of the last
// plan, and `auto` when the mode of the composer is auto and the Face ID unlock of the
// app is on at the tap on Send (features/navigation/model/app-lock.ts).

import { useMutation, useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";

import type { MachineCaller } from "@/lib/query/machine-caller";
import { type AppLock, selectLockOn } from "@/features/navigation/model/app-lock";
import type { AppAttach } from "@/features/navigation/model/app-services";
import type { PreferencesStore } from "@/features/settings/model/preferences";
import {
  interruptMutationOptions,
  modelsQueryOptions,
  steerMutationOptions,
  type InterruptInput,
  type SteerInput,
} from "./conversation-queries";
import { outboxFiles } from "./model/attachments";
import { composerDraftKey, conversationIdOf, type ComposerTarget } from "./model/composer-state";
import type { Drafts } from "./model/drafts";
import type { ModelChoices } from "./model/model-choices";
import type { Outbox, OutboxEntry } from "./model/outbox";
import { sendPrompt } from "./model/send-prompt";
import type { Voice } from "./model/voice";

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
  /** The Face ID unlock of the app: auto from the phone needs it (round 9). */
  readonly lock: Pick<AppLock, "store" | "facts">;
  /** The pickers and the files of the drafts; null in a build without them. */
  readonly attach: AppAttach | null;
  /** Voice input; null in a build without efr-speech. */
  readonly voice: Voice | null;
}

/** The actions of one composer. */
export interface ComposerActions {
  /** Sends `text` and the files of the draft as a prompt through the outbox, and clears the draft. */
  send(text: string): OutboxEntry;
  /** Queues `text` and the files of the draft behind the running turn, and clears the draft. */
  queue(text: string): OutboxEntry;
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

  const enqueue = (text: string): OutboxEntry => {
    const lockOn = selectLockOn(deps.lock.store.get());
    // NOTE: the composer takes no submit while a photo is in the work, so `take` gives
    // the files here.
    const files = deps.attach?.attachments.take(key) ?? [];
    const entry = sendPrompt(
      { outbox, modelChoices: deps.modelChoices, lockOn },
      list,
      target,
      text,
      outboxFiles(files),
    );
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
