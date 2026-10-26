// The copy button (design 3.5 and 3.17): it copies with expo-clipboard, plays the light
// haptic, and shows that it copied for 1.5 s (design item 8.8: 1.5 s everywhere).
//
// - `code`: the copy button of a code block header, 36 x 32, doc.on.doc 15 pt in
//   text_muted. It shows "✓ copied" in mono 11 state_success with a check icon. While
//   the fence still streams it is disabled, and its icon is text_decorative.
// - `circle`: the 40 pt copy button of the pairing command box, icon 17 pt in
//   text_muted. The icon turns to a check.

import { useRef, useState } from "react";
import { Pressable, Text } from "react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";

import { cn } from "./cn";
import { SymbolIcon } from "./symbol";

/** How long the button shows that it copied. */
export const COPIED_MS = 1500;

/** The look of the button. */
export type CopyButtonVariant = "code" | "circle";

const FRAME = {
  code: "h-32 min-w-36 flex-row items-center justify-center gap-4 px-6",
  circle: "w-40 h-40 items-center justify-center rounded-full",
} as const satisfies Record<CopyButtonVariant, string>;

const ICON_SIZE = { code: 15, circle: 17 } as const satisfies Record<CopyButtonVariant, number>;

export interface CopyButtonProps {
  readonly variant: CopyButtonVariant;
  /** The text to copy. A function runs only on the tap. */
  readonly text: string | (() => string);
  /** The VoiceOver label, such as "Copy code" or "Copy command". */
  readonly label: string;
  /** True while there is nothing whole to copy yet (a fence that still streams). */
  readonly disabled?: boolean;
}

/** A button that copies a text and shows "copied" for a moment. */
export function CopyButton(props: CopyButtonProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const disabled = props.disabled === true;
  const variant = props.variant;
  const press = (): void => {
    const text = typeof props.text === "function" ? props.text() : props.text;
    void copyText(text);
    haptic("light");
    setCopied(true);
    if (timer.current !== undefined) {
      clearTimeout(timer.current);
    }
    timer.current = setTimeout(() => {
      timer.current = undefined;
      setCopied(false);
    }, COPIED_MS);
  };
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={copied ? "Copied" : props.label}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={press}
      className={cn(FRAME[variant], "active:opacity-60")}
    >
      {copied && variant === "code" ? (
        <Text className="text-mono-xs text-state-success">✓ copied</Text>
      ) : null}
      <SymbolIcon
        name={copied ? "checkmark" : "doc.on.doc"}
        size={ICON_SIZE[variant]}
        tone={copied ? "state_success" : disabled ? "text_decorative" : "text_muted"}
      />
    </Pressable>
  );
}
