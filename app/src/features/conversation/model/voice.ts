// Voice input of the composer (spec 7.3, board S-Voice): speech becomes text in the draft
// on the iPhone, with efr-speech (SpeechAnalyzer of iOS 26). Voice edits the draft and
// never sends a prompt.
//
// - The controller lives in the runtime, not in React: the composer taps it, and the
//   events of the module feed it. One recording runs at a time, for one draft.
// - The states: off (no support), idle, preparing (the microphone question, and the
//   speech model on the first use of a language), recording, transcribing (after the
//   stop, until the last words come).
// - The text goes in at the cursor, after the text that was there, and each result
//   changes only the words of this recording. When the user changed those words while it
//   records, the next words go at the end of the draft.
// - A recording stops after MAX_RECORDING_MS, at the stop button, or when a phone call or
//   Siri takes the microphone; the text so far stays. A haptic marks the start and the
//   stop.
// - A line tells what went wrong: the microphone is off for efr, nothing was heard, a
//   call stopped it, or iOS failed. It goes at the next tap of the microphone or at Send.
//
// Pure TypeScript on TanStack Store. The module, the drafts, the clock and the haptic
// come in.

import { createStore, type Readable } from "@tanstack/store";

import type { Clock, Timer } from "@/lib/net/clock";
import type { EfrSpeechNative } from "../../../../modules/efr-speech";
import type { ComposerTrailing } from "./composer-state";
import type { DraftKey, Drafts } from "./drafts";

/** The longest recording (spec 7.3). */
export const MAX_RECORDING_MS = 5 * 60 * 1000;

/** The code of the error of `start` when iOS denies the microphone (efr-speech). */
export const MICROPHONE_DENIED = "ERR_MICROPHONE_DENIED";

/** What went wrong with the last recording, for its line. */
export type VoiceProblem = "denied" | "silent" | "interrupted" | "failed";

/** The state of voice input. */
export type VoiceState =
  /** This iPhone has no speech support for its language: no microphone shows. */
  | { readonly kind: "off" }
  | { readonly kind: "idle"; readonly problem: VoiceProblem | undefined }
  /** `model` while iOS gets the speech model of the language. */
  | { readonly kind: "preparing"; readonly key: DraftKey; readonly model: boolean }
  /** `startedAt` is wall-clock time in ms, as the composer reads the time with `useNow`. */
  | { readonly kind: "recording"; readonly key: DraftKey; readonly startedAt: number }
  | { readonly kind: "transcribing"; readonly key: DraftKey };

/** What the controller needs from the app. */
export interface VoiceDeps {
  readonly speech: EfrSpeechNative;
  readonly drafts: Pick<Drafts, "text" | "set">;
  readonly clock: Clock;
  /** The native haptic at the start and the stop. */
  readonly haptic: () => void;
  readonly onError: (error: unknown) => void;
}

/** Voice input of the app. */
export interface Voice {
  readonly store: Readable<VoiceState>;
  /** Starts a recording into the draft of `key` at `cursor`, a UTF-16 index of its text. */
  start(key: DraftKey, cursor: number): void;
  /** Stops the recording; the last words follow. */
  stop(): void;
  /** Takes the line of the last problem away, as at Send. */
  dismiss(): void;
  /** Stops the listeners. */
  dispose(): void;
}

/** The line of each problem, above the status line (board S-Voice 5). */
export const VOICE_LINES: Readonly<Record<VoiceProblem, string>> = {
  denied: "The microphone is off for efr. Tap to open Settings.",
  silent: "efr heard nothing. Nothing was added.",
  interrupted: "A call stopped the recording. The text so far is in the message.",
  failed: "The recording stopped with an error. The text so far is in the message.",
};

/** The quiet line while iOS gets the speech model of the language. */
export const MODEL_LINE = "Getting the speech model";

/** The VoiceOver label of the stop button after `seconds` of recording. */
export function stopLabel(seconds: number): string {
  return `Stop recording, ${seconds} ${seconds === 1 ? "second" : "seconds"}`;
}

const OFF: VoiceState = { kind: "off" };
const IDLE: VoiceState = { kind: "idle", problem: undefined };

/** Voice input of a build without efr-speech: off. */
export const NO_VOICE: Readable<VoiceState> = createStore<VoiceState>(OFF);

/** The state of the draft of `key`: what its composer shows. */
export function voiceOf(state: VoiceState, key: DraftKey): VoiceState {
  return "key" in state && state.key !== key ? IDLE : state;
}

/** True while a recording writes into a draft: Send is off then, so voice never sends. */
export function voiceWrites(state: VoiceState): boolean {
  return state.kind === "preparing" || state.kind === "recording" || state.kind === "transcribing";
}

/** The trailing action of a composer while voice writes into its draft: nothing goes. */
export function trailingWhileVoice(
  trailing: ComposerTrailing,
  state: VoiceState,
): ComposerTrailing {
  if (!voiceWrites(state)) {
    return trailing;
  }
  if (trailing.kind === "send") {
    return { kind: "send", enabled: false };
  }
  return trailing.kind === "stop" ? { kind: "stop", capsule: null } : trailing;
}

/** The `code` of an error of a native module, such as `ERR_MICROPHONE_DENIED`. */
function codeOf(error: unknown): unknown {
  return typeof error === "object" && error !== null && "code" in error ? error.code : undefined;
}

/** True when `text` ends with a blank, or is empty: the words then need no space before. */
function joins(text: string): boolean {
  return text === "" || /\s$/.test(text);
}

/** Makes the controller. It checks the support of the iPhone once, at once. */
export function createVoice(deps: VoiceDeps): Voice {
  const { speech, drafts, clock } = deps;
  const store = createStore<VoiceState>(OFF);
  let limit: Timer | undefined;
  // Where the words of this recording stand in the draft, and what they are now. Each
  // event holds the whole text so far: `skip` of it went into the draft before the user
  // changed those words, and `last` is the text of the last event.
  let at = 0;
  let words = "";
  let skip = 0;
  let last = "";
  let heard = false;

  const set = (next: VoiceState): void => {
    store.setState(() => next);
  };

  const keyOf = (): DraftKey | undefined => {
    const state = store.state;
    return "key" in state ? state.key : undefined;
  };

  const write = (text: string): void => {
    const key = keyOf();
    if (key === undefined) {
      return;
    }
    heard = heard || text.trim() !== "";
    let draft = drafts.text(key);
    if (draft.slice(at, at + words.length) !== words) {
      // The user changed the words of this recording: only the new words go, at the end.
      at = draft.length;
      words = "";
      skip = last.length;
    }
    last = text;
    const fresh = text.slice(skip).trimStart();
    const before = draft.slice(0, at);
    const after = draft.slice(at + words.length);
    // A space before the words, and one after them when a word follows.
    const space = joins(before) ? "" : " ";
    const next = fresh === "" ? "" : `${space}${fresh}${/^[\p{L}\p{N}]/u.test(after) ? " " : ""}`;
    draft = `${before}${next}${after}`;
    words = next;
    drafts.set(key, draft);
  };

  const end = (problem: VoiceProblem | undefined): void => {
    limit?.cancel();
    limit = undefined;
    // A stop of the user had its haptic at the tap.
    if (store.state.kind === "recording") {
      deps.haptic();
    }
    set({ kind: "idle", problem });
  };

  const subscriptions = [
    speech.addListener("phase", (event) => {
      const state = store.state;
      if (state.kind === "transcribing") {
        // The user stopped before the microphone ran: the module stops now.
        speech.stop().catch(deps.onError);
        return;
      }
      if (state.kind !== "preparing") {
        return;
      }
      if (event.phase === "model") {
        set({ kind: "preparing", key: state.key, model: true });
        return;
      }
      deps.haptic();
      set({ kind: "recording", key: state.key, startedAt: clock.now() });
      limit = clock.setTimer(MAX_RECORDING_MS, () => stop());
    }),
    speech.addListener("transcript", (event) => write(event.text)),
    speech.addListener("end", (event) => {
      if (keyOf() === undefined) {
        return;
      }
      if (event.reason === "interrupted") {
        end("interrupted");
      } else if (event.reason === "failed") {
        deps.onError(new Error(event.message ?? "the recording failed"));
        end("failed");
      } else {
        end(heard ? undefined : "silent");
      }
    }),
  ];

  const stop = (): void => {
    const state = store.state;
    if (state.kind !== "recording" && state.kind !== "preparing") {
      return;
    }
    if (state.kind === "recording") {
      deps.haptic();
    }
    set({ kind: "transcribing", key: state.key });
    speech.stop().catch(deps.onError);
  };

  const check = async (): Promise<void> => {
    if (await speech.isAvailable()) {
      set(IDLE);
    }
  };
  check().catch(deps.onError);

  return {
    store,
    start(key: DraftKey, cursor: number): void {
      if (store.state.kind !== "idle") {
        return;
      }
      at = Math.max(0, Math.min(cursor, drafts.text(key).length));
      words = "";
      skip = 0;
      last = "";
      heard = false;
      set({ kind: "preparing", key, model: false });
      speech.start().catch((error: unknown) => {
        const state = store.state.kind;
        if (state === "idle") {
          // The end of the recording came first.
          return;
        }
        if (state === "transcribing") {
          // The user stopped before the recording ran.
          end(undefined);
          return;
        }
        const denied = codeOf(error) === MICROPHONE_DENIED;
        if (!denied) {
          deps.onError(error);
        }
        end(denied ? "denied" : "failed");
      });
    },
    stop,
    dismiss(): void {
      if (store.state.kind === "idle" && store.state.problem !== undefined) {
        set(IDLE);
      }
    },
    dispose(): void {
      limit?.cancel();
      for (const subscription of subscriptions) {
        subscription.remove();
      }
    },
  };
}
