// The answer row of the composer (S-Ask 3): while the user types an answer to a question
// of the model, it takes the place of the strip and the status line. "Answer" in yolk,
// the question on one line, and an xmark (44 pt) that leaves the answer state. The fixed
// hint stands under the field (`ComposerAnswerHint`).
//
// The row takes the focus to the field when it shows, so "Type an answer" opens the
// keyboard. A ref callback does it, so the row needs no effect.

import { use } from "react";
import { Pressable, Text, View, type HostInstance } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { SymbolIcon } from "@/components/ui/symbol";

import { ANSWER_HINT } from "../../model/question";
import { ComposerContext, ComposerInputRefContext } from "./composer-context";

/** The answer row, or nothing while the composer sends a message. */
export function ComposerAnswerRow() {
  const model = use(ComposerContext);
  const inputRef = use(ComposerInputRefContext);
  const answering = model?.answering;
  if (answering === undefined) {
    return null;
  }
  const focusField = (view: HostInstance | null): void => {
    if (view !== null) {
      inputRef?.current?.focus();
    }
  };
  return (
    <View
      ref={focusField}
      testID="composer-answer-row"
      className="min-h-44 flex-row items-center gap-8 pr-0 pl-16"
    >
      <Text className="text-row-detail text-accent-primary">Answer</Text>
      <BodyText size="rowDetail" tone="text_primary" numberOfLines={1} className="flex-1">
        {answering.question}
      </BodyText>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel="Leave the answer"
        accessibilityHint="The text stays as a message."
        onPress={answering.leave}
        className="h-hit w-hit items-center justify-center active:opacity-60"
      >
        <SymbolIcon name="xmark" size={13} weight="semibold" tone="text_muted" />
      </Pressable>
    </View>
  );
}

/** The fixed hint under the field while the user types an answer. */
export function ComposerAnswerHint() {
  const model = use(ComposerContext);
  if (model?.answering === undefined) {
    return null;
  }
  return (
    <BodyText size="rowDetail" tone="text_subtle" className="px-16 pb-8">
      {ANSWER_HINT}
    </BodyText>
  );
}
