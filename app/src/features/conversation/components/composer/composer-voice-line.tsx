// The one line of voice input above the status line (board S-Voice 2 and 5): "Getting the
// speech model" in text_subtle while iOS gets it, or the problem of the last recording in
// state_error until the next tap of the microphone or Send. The line of a denied
// microphone is a button that opens the iOS settings of efr.

import { use } from "react";
import { Linking, Pressable } from "react-native";

import { BodyText } from "@/components/ui/body-text";

import { MODEL_LINE, VOICE_LINES } from "../../model/voice";
import { ComposerContext } from "./composer-context";

/** The line of voice input, or nothing. */
export function ComposerVoiceLine() {
  const state = use(ComposerContext)?.voice?.state;
  if (state?.kind === "preparing" && state.model) {
    return (
      <BodyText
        testID="composer-voice-line"
        size="caption"
        tone="text_subtle"
        className="px-16 pt-8"
      >
        {MODEL_LINE}
      </BodyText>
    );
  }
  if (state?.kind !== "idle" || state.problem === undefined) {
    return null;
  }
  const line = (
    <BodyText testID="composer-voice-line" size="caption" tone="state_error" className="px-16 pt-8">
      {VOICE_LINES[state.problem]}
    </BodyText>
  );
  if (state.problem !== "denied") {
    return line;
  }
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={VOICE_LINES.denied}
      onPress={() => {
        Linking.openSettings().catch(() => undefined);
      }}
    >
      {line}
    </Pressable>
  );
}
