// The native side of voice input: efr-speech and the haptic of a start and a stop. A build
// without efr-speech gives null, and the composer then shows no microphone
// (lib/native/native-parts.ts).

import type { VoiceEffects } from "@/features/navigation/model/app-services";
import { haptic } from "@/lib/native/haptics";
import { NATIVE_PARTS } from "@/lib/native/native-parts";
import { EfrSpeech } from "../../../../modules/efr-speech";

/** The voice effects of the app, or null in a build without them. */
export function nativeVoice(): VoiceEffects | null {
  const speech = EfrSpeech;
  if (!NATIVE_PARTS.voice || speech === null) {
    return null;
  }
  return { speech, haptic: () => haptic("light") };
}
