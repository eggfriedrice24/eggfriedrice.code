// The local Expo module of voice input. Autolinking finds it in app/modules on an EAS or
// Xcode build. A build without the module gives null here, and the app then shows no
// microphone. `expo.autolinking.exclude` in app/package.json can take it out of a build.

import { requireOptionalNativeModule } from "expo";

import type { EfrSpeechNative } from "./src/EfrSpeech.types";

export type {
  EfrSpeechEvents,
  EfrSpeechNative,
  SpeechEndReason,
  SpeechPhase,
} from "./src/EfrSpeech.types";

/** The native module, or null when this build does not have it. */
export const EfrSpeech: EfrSpeechNative | null =
  requireOptionalNativeModule<EfrSpeechNative>("EfrSpeech");
