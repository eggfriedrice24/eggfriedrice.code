// The filled yolk button (UIButton.Configuration.filled() in the design): "Scan code",
// "Start", "Scan again" and the other actions of pairing, and Allow of an approval block.
// The label and the icon are text_on_accent. While its action runs, the button shows
// the braille cell in place of the label and keeps its size (design 3.11).

import { Pressable, Text } from "react-native";

import { BrailleCell } from "./braille-cell";
import {
  BUTTON_BASE,
  BUTTON_DISABLED,
  BUTTON_FRAME,
  BUTTON_GROW,
  BUTTON_LABEL,
  type ButtonSize,
} from "./button-classes";
import { cn } from "./cn";
import { SymbolIcon, type SymbolName } from "./symbol";

export interface PrimaryButtonProps {
  readonly label: string;
  readonly onPress: () => void;
  /** The default is `large`, the size of the pairing screens. */
  readonly size?: ButtonSize;
  /** An SF Symbol before the label, such as qrcode.viewfinder for "Scan code". */
  readonly symbol?: SymbolName;
  /** True while the action runs: the braille cell replaces the label. */
  readonly pending?: boolean;
  /** The VoiceOver label while pending, such as "Allowing". */
  readonly pendingLabel?: string;
  readonly disabled?: boolean;
  /** True to take an equal share of the row (Allow next to Deny). */
  readonly grow?: boolean;
}

/** The filled yolk button. */
export function PrimaryButton(props: PrimaryButtonProps) {
  const size = props.size ?? "large";
  const pending = props.pending === true;
  const disabled = props.disabled === true;
  const symbol = props.symbol;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={pending ? (props.pendingLabel ?? props.label) : props.label}
      accessibilityState={{ disabled: disabled || pending, busy: pending }}
      disabled={disabled || pending}
      onPress={props.onPress}
      className={cn(
        BUTTON_BASE,
        BUTTON_FRAME[size],
        "bg-accent-primary active:opacity-80",
        disabled && BUTTON_DISABLED,
        props.grow === true && BUTTON_GROW,
      )}
    >
      {pending ? (
        <BrailleCell size="button" tint="on_accent" />
      ) : (
        <>
          {symbol === undefined ? null : (
            <SymbolIcon name={symbol} size={20} tone="text_on_accent" />
          )}
          <Text numberOfLines={1} className={cn(BUTTON_LABEL[size], "text-text-on-accent")}>
            {props.label}
          </Text>
        </>
      )}
    </Pressable>
  );
}
