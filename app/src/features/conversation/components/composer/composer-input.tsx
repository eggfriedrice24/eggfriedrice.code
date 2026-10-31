// Slot C of the composer, the text input (design 3.3): a multiline UITextView in SF Pro
// 15/21, text_primary on no fill, with padding 9 16 10. It has no border: the glass pill
// of the root is the shape. It grows from 1 to 6 lines (40 to 145 pt), then scrolls
// inside.
// Return inserts a newline; the trailing button sends. The caret is yolk.
//
// - While a send is in flight, the text shows in text_subtle and the changes are dropped.
//   The native input stays editable, so the keyboard stays up.
// - Disconnected: the input takes no text, is at 60% and its placeholder says why.

import { use } from "react";
import { TextInput } from "react-native";

import { cn } from "@/components/ui/cn";
import { roles } from "@/theme/roles";

import { ComposerContext, ComposerInputRefContext } from "./composer-context";

/** The text input of the composer. It reads the draft through the composer. */
export function ComposerInput() {
  const model = use(ComposerContext);
  const inputRef = use(ComposerInputRefContext);
  if (model === null) {
    return null;
  }
  const kind = model.state.kind;
  return (
    <TextInput
      ref={inputRef ?? undefined}
      testID="composer-input"
      accessibilityLabel="Message"
      multiline
      submitBehavior="newline"
      keyboardAppearance="dark"
      value={model.value}
      onChangeText={model.setText}
      editable={kind !== "disconnected"}
      placeholder={model.placeholder}
      placeholderTextColor={roles.text_subtle}
      selectionColor={roles.accent_primary}
      cursorColor={roles.accent_primary}
      className={cn(
        "min-h-field max-h-field-max flex-1 px-16 pt-9 pb-10 text-input",
        kind === "sending" ? "text-text-subtle" : "text-text-primary",
        kind === "disconnected" && "opacity-60",
      )}
    />
  );
}
