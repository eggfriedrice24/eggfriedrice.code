// Slot C of the composer, the text input (design 3.3): a multiline UITextView in SF Pro
// 15/21, text_primary on no fill. It has no border: the glass pill of the root is the
// shape. Return inserts a newline; the trailing button sends. The caret is yolk.
//
// - At rest (no focus) it sits between the attach and voice buttons: one line (40 pt,
//   padding 9 4 10), no scroll, so a long draft shows its first line in the pill.
// - Focused, it takes the full width under the status line, with the 16 pt sides of the
//   line, so the text and the placeholder start at the top left. It is at least 72 pt
//   (about 3 lines, as t3code), grows to 6 lines (145 pt), then scrolls inside.
// - The placeholder is 13 pt, smaller than the 15 pt text: the empty field takes the
//   `inputEmpty` style, with the same line height, so the field does not jump.
// - While a send is in flight, the text shows in text_subtle and the changes are dropped.
//   The native input stays editable, so the keyboard stays up.
// - Disconnected: the input takes no text, is at 60% and its placeholder says why.

import { use } from "react";
import { TextInput } from "react-native";

import { cn } from "@/components/ui/cn";
import { roles } from "@/theme/roles";

import { ANSWER_PLACEHOLDER } from "../../model/question";
import {
  ComposerContext,
  ComposerCursorContext,
  ComposerInputRefContext,
  ComposerSizeContext,
} from "./composer-context";

/** The text input of the composer. It reads the draft through the composer. */
export function ComposerInput() {
  const model = use(ComposerContext);
  const inputRef = use(ComposerInputRefContext);
  const cursorRef = use(ComposerCursorContext);
  const size = use(ComposerSizeContext);
  if (model === null) {
    return null;
  }
  const kind = model.state.kind;
  return (
    <TextInput
      ref={inputRef ?? undefined}
      testID="composer-input"
      accessibilityLabel={model.answering === undefined ? "Message" : ANSWER_PLACEHOLDER}
      multiline
      submitBehavior="newline"
      keyboardAppearance="dark"
      value={model.value}
      onChangeText={model.setText}
      onSelectionChange={(event) => {
        if (cursorRef !== null) {
          cursorRef.current = event.nativeEvent.selection.start;
        }
      }}
      editable={kind !== "disconnected"}
      onFocus={() => size.setFocused(true)}
      onBlur={() => size.setFocused(false)}
      scrollEnabled={size.expanded}
      placeholder={model.placeholder}
      placeholderTextColor={roles.text_subtle}
      selectionColor={roles.accent_primary}
      cursorColor={roles.accent_primary}
      className={cn(
        model.value.length === 0 ? "text-input-empty" : "text-input",
        size.expanded
          ? "min-h-field-focused max-h-field-max px-16 pt-2 pb-6"
          : "min-h-field max-h-field flex-1 px-4 pt-9 pb-10",
        kind === "sending" ? "text-text-subtle" : "text-text-primary",
        kind === "disconnected" && "opacity-60",
      )}
    />
  );
}
