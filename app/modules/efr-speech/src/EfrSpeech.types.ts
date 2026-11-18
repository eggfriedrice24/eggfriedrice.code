// The functions and the events of the native module EfrSpeech
// (ios/EfrSpeechModule.swift). The module records only while the app asks it to, and it
// never writes the audio to a file.

import type { NativeModule } from "expo";

/** `model` while iOS gets the speech model of the language, then `recording`. */
export type SpeechPhase = "model" | "recording";

/** Why a recording ended: `stop`, a phone call or Siri, or an error. */
export type SpeechEndReason = "stopped" | "interrupted" | "failed";

/** The events of the module. */
export type EfrSpeechEvents = {
  phase: (event: { readonly phase: SpeechPhase }) => void;
  /** The whole text so far: the final parts, then the newest part that can still change. */
  transcript: (event: { readonly text: string }) => void;
  /** The last event of a recording, after its last `transcript`. */
  end: (event: { readonly reason: SpeechEndReason; readonly message?: string | null }) => void;
};

/** The native module, as `requireOptionalNativeModule("EfrSpeech")` gives it. */
export interface EfrSpeechNative extends NativeModule<EfrSpeechEvents> {
  /** True on iOS 26 and later when this iPhone turns speech into text in its language. */
  isAvailable(): Promise<boolean>;
  /**
   * Asks for the microphone, gets the speech model when it is not on the iPhone yet, and
   * records. It resolves when the recording runs.
   */
  start(): Promise<void>;
  /** Stops the recording. With no recording, it does nothing. */
  stop(): Promise<void>;
}
