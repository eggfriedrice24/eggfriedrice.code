// The line of a message that did not pass the check of the owner (S-ComposerR7, AU-Auto
// 5): one line in the strip slot of the composer, above the status line. "Not sent." in
// text_primary, then why and how to try again in text_muted, 13/18. VoiceOver reads it
// when it shows. model/owner-check.ts has the words.

import { use } from "react";
import { Text, View } from "react-native";

import { ComposerContext } from "./composer-context";

/** The not-sent line, or nothing. */
export function ComposerNotSent() {
  const model = use(ComposerContext);
  const line = model?.notSent;
  if (line === undefined) {
    return null;
  }
  return (
    <View
      testID="composer-not-sent"
      accessible
      accessibilityLiveRegion="polite"
      role="alert"
      className="px-16 pt-8"
    >
      <Text className="text-reason text-text-muted">
        <Text className="text-text-primary">{line.head}</Text> {line.rest}
      </Text>
    </View>
  );
}
