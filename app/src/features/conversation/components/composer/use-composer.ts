// The model of one composer: it reads the connection, the draft, the outbox and the send
// mode with narrow selectors, gives them to the pure functions of
// model/composer-state.ts, and joins the actions. `Composer.Root` calls it once and
// gives the result to the slots through the context.
//
// A message in auto (round 9, S-ComposerR7 13, AU-Auto r9 4): a send or a queue goes in
// auto when the mode of this conversation is auto AND the Face ID unlock of the app is on.
// The unlock is the check, so Send asks nothing more. With the unlock off the mode is
// cautious. A steer names no mode.
//
// The files of the draft (S-Attach): the plus shows when the build has the pickers and
// efrd sent the capability `attachments`. A draft with a file that is not ready sends
// nothing, and while a turn runs it only queues.
//
// Voice (S-Voice): the microphone shows when the build has efr-speech and the iPhone
// turns speech into text in its language. While a recording writes into the draft, Send
// is off, so voice never sends; a Send takes the line of a problem away.

import { useSelector } from "@tanstack/react-store";
import { createStore, type Readable } from "@tanstack/store";
import { useState } from "react";

import { devWarn } from "@/lib/dev/dev-warn";
import type { AttachmentLimits } from "@/lib/protocol/generated";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import { quickLook, type AppAttach } from "@/features/navigation/model/app-services";
import type { SendMode } from "@/features/settings/model/preferences";
import {
  NO_ATTACHMENTS,
  createLimitsSelector,
  selectDraftFiles,
  type AttachSource,
} from "../../model/attachments";
import {
  draftTiles,
  tilesReady,
  trailingWithFiles,
  type TileView,
} from "../../model/attachments-tiles";
import type { DraftKey } from "../../model/drafts";
import {
  NO_VOICE,
  trailingWhileVoice,
  voiceOf,
  voiceWrites,
  type Voice,
  type VoiceState,
} from "../../model/voice";
import {
  composerDraftKey,
  composerEditable,
  composerPlaceholder,
  composerState,
  composerTrailing,
  createLinkSelector,
  inFlightText,
  ignoreReason,
  type ComposerTarget,
} from "../../model/composer-state";
import { selectDraft } from "../../model/drafts";
import { effectiveMode, selectMode, type PhoneMode } from "../../model/model-choices";
import type { OutboxEntry } from "../../model/outbox";
import { selectLockOn } from "@/features/navigation/model/app-lock";
import { ANSWER_PLACEHOLDER, answerText } from "../../model/question";
import { useComposerActions, type ComposerDeps } from "../../use-composer-actions";
import type {
  ComposerAnswer,
  ComposerAttach,
  ComposerModel,
  ComposerVoice,
} from "./composer-context";

/** The typed answer of a composer that answers no question. */
const NO_ANSWER: Readable<string> = createStore("");

/** What a composer reads from its screen. */
export interface ComposerOptions {
  readonly deps: ComposerDeps;
  readonly target: ComposerTarget;
  /** The connection atom of the machine's supervisor. */
  readonly connection: Readable<MachineConnection>;
  /** The name of the machine, for "laptop is offline". */
  readonly machineName: string;
  /** True when no machine of the phone answers: "Tailscale is not connected". */
  readonly tailnetDown?: boolean;
  /** The running turn of the conversation, or undefined when none runs. */
  readonly runningTurnId?: string;
  /**
   * Called with the outbox entry of each prompt that the composer sends. On the blank
   * root, the screen pushes the new conversation here.
   */
  readonly onSend?: (entry: OutboxEntry) => void;
  /** The question of the model that Send answers now (S-Ask 3); none for a message. */
  readonly answering?: ComposerAnswer;
}

/** The model of the composer of `options.target`. */
export function useComposer(options: ComposerOptions): ComposerModel {
  const { deps, target, runningTurnId, onSend } = options;
  const [selectLink] = useState(createLinkSelector);
  const link = useSelector(options.connection, selectLink);
  const key = composerDraftKey(target);
  const text = useSelector(deps.drafts.store, selectDraft(key));
  const typed = useSelector(options.answering?.draft ?? NO_ANSWER, (answer) => answer);
  const inFlight = useSelector(deps.outbox.store, (snapshot) => inFlightText(snapshot, target));
  const sendMode = useSelector(deps.preferences.store, (preferences) => preferences.sendMode);
  const stored = useSelector(deps.modelChoices.store, selectMode(key));
  const lockOn = useSelector(deps.lock.store, selectLockOn);
  // NOTE: auto only while the Face ID unlock is on (round 9); the unlock is the check.
  const mode = effectiveMode(stored, lockOn);
  const [selectLimits] = useState(createLimitsSelector);
  const limits = useSelector(options.connection, selectLimits);
  const files = useSelector(
    deps.attach?.attachments.store ?? NO_ATTACHMENTS,
    selectDraftFiles(key),
  );
  const allVoice = useSelector(deps.voice?.store ?? NO_VOICE, (voice) => voice);
  const actions = useComposerActions(deps, target, runningTurnId);
  const state = composerState({
    isBlankRoot: target.isBlankRoot,
    link,
    runningTurnId,
    text,
    sending: inFlight !== undefined || actions.steering,
    sendMode,
  });
  const tiles = draftTiles(files, limits);
  const attach = composerAttach(deps.attach, key, tiles, limits);
  const voiceState = voiceOf(allVoice, key);
  const voice = composerVoice(deps.voice, voiceState, allVoice, (cursor) =>
    deps.voice?.start(key, cursor ?? text.length),
  );

  // NOTE: Each send runs before the call of `onSend`. `onSend?.(actions.send(text))` does
  // not send at all when the screen gives no `onSend`, as a conversation screen does.
  const submit = (pickedMode: SendMode): void => {
    // A steer takes no file, so a draft with files queues.
    const kind = sendKindOf(state, tiles.length > 0 ? "queue" : pickedMode);
    if (kind === undefined) {
      devWarn(`the composer of ${target.id} ignores a submit: ${ignoreReason(state)}`);
      return;
    }
    if (!tilesReady(tiles) || voiceWrites(voiceState)) {
      devWarn(`the composer of ${target.id} ignores a submit: a file or voice is not ready`);
      return;
    }
    deps.voice?.dismiss();
    // A steer names no mode: it joins the turn that runs, in its own mode.
    if (kind === "steer") {
      actions.steer(text);
      return;
    }
    // NOTE: the prompt reads the mode and the unlock again at the tap (send-prompt.ts).
    const entry = kind === "send" ? actions.send(text) : actions.queue(text);
    onSend?.(entry);
  };

  const model: ComposerModel = {
    answering: undefined,
    attach,
    queueOnly: tiles.length > 0,
    voice,
    state,
    trailing: trailingWhileVoice(trailingWithFiles(composerTrailing(state), tiles), voiceState),
    isBlankRoot: target.isBlankRoot,
    value: state.kind === "sending" ? (inFlight ?? text) : text,
    placeholder: composerPlaceholder(state, {
      machineName: options.machineName,
      tailnetDown: options.tailnetDown === true,
    }),
    sendMode,
    stopping: actions.stopping,
    setText: (next: string): void => {
      if (composerEditable(state)) {
        deps.drafts.set(key, next);
      }
    },
    submit: (): void => {
      submit(sendMode);
    },
    pick: (picked: SendMode): void => {
      deps.preferences.setSendMode(picked);
      submit(picked);
    },
    stop: (): void => {
      if (!actions.stopping) {
        actions.stop();
      }
    },
    mode,
    setMode: (next: PhoneMode): void => {
      deps.modelChoices.setMode(key, next);
    },
  };
  const answering = options.answering;
  if (answering === undefined) {
    return model;
  }
  // S-Ask 3: the field holds the answer, and Send answers the question. Stop and the
  // Steer/Queue capsule wait until the user leaves the answer state.
  const answer = answerText(typed);
  const online = state.kind !== "disconnected";
  return {
    ...model,
    answering,
    attach: undefined,
    queueOnly: false,
    voice: undefined,
    trailing: answering.sending
      ? { kind: "sending", stop: false }
      : { kind: "send", enabled: answer !== undefined && online },
    value: typed,
    placeholder: ANSWER_PLACEHOLDER,
    setText: (next: string): void => {
      if (online && !answering.sending) {
        answering.setText(next);
      }
    },
    submit: (): void => {
      if (answer !== undefined && online && !answering.sending) {
        answering.send(answer);
      }
    },
  };
}

/**
 * The files of the draft of `key`, or undefined when the plus does not show: no pickers in
 * the build, or no capability `attachments` of efrd.
 */
function composerAttach(
  attach: AppAttach | null,
  key: DraftKey,
  tiles: readonly TileView[],
  limits: AttachmentLimits | undefined,
): ComposerAttach | undefined {
  if (attach === null || limits === undefined) {
    return undefined;
  }
  const { native, attachments } = attach;
  const room = Math.max(0, limits.max_per_message - tiles.length);
  return {
    tiles,
    limits,
    room,
    pick: (source: AttachSource): void => {
      native
        .pick(source, room)
        .then((picked) => attachments.add(key, picked, limits.max_edge_px))
        .catch((error: unknown) => {
          devWarn(`the picker of ${source} failed: ${String(error)}`);
        });
    },
    remove: (id: string): void => attachments.remove(key, id),
    quickLook: (id: string): void => {
      const file = selectDraftFiles(key)(attachments.store.get()).find((held) => held.id === id);
      if (file !== undefined) {
        quickLook(attach, file.uri, file.name);
      }
    },
  };
}

/**
 * Voice input of the draft, or undefined when no microphone shows: no efr-speech in the
 * build, or no speech support on this iPhone.
 */
function composerVoice(
  voice: Voice | null,
  state: VoiceState,
  all: VoiceState,
  start: (cursor: number | undefined) => void,
): ComposerVoice | undefined {
  if (voice === null || state.kind === "off") {
    return undefined;
  }
  return {
    state,
    busy: state.kind === "idle" && all.kind !== "idle",
    start,
    stop: () => voice.stop(),
  };
}

/** What a tap on Send does. */
type SendKind = "send" | "steer" | "queue";

/** What a tap on Send does in `state` with the send mode `mode`, or none. */
function sendKindOf(state: ReturnType<typeof composerState>, mode: SendMode): SendKind | undefined {
  if (state.kind === "typing") {
    return "send";
  }
  if (state.kind === "running" && state.hasText) {
    return mode === "steer" ? "steer" : "queue";
  }
  return undefined;
}
