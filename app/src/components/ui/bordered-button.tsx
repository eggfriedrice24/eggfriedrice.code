// The bordered button (UIButton .bordered in the design): Deny of an approval block and
// Cancel of the pairing confirm. A 1 pt border_default edge, text_bright label, no fill.
// Deny is not red (design 3.11).

import { Pressable, Text } from "react-native";

import {
  BUTTON_BASE,
  BUTTON_DISABLED,
  BUTTON_FRAME,
  BUTTON_GROW,
  BUTTON_LABEL,
  type ButtonSize,
} from "./button-classes";
import { cn } from "./cn";

export interface BorderedButtonProps {
  readonly label: string;
  readonly onPress: () => void;
  /** The default is `large`, the size of the pairing screens. */
  readonly size?: ButtonSize;
  readonly disabled?: boolean;
  /** True to take an equal share of the row (Deny next to Allow). */
  readonly grow?: boolean;
}

/** The bordered button. */
export function BorderedButton(props: BorderedButtonProps) {
  const size = props.size ?? "large";
  const disabled = props.disabled === true;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.label}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={props.onPress}
      className={cn(
        BUTTON_BASE,
        BUTTON_FRAME[size],
        "border border-border-default active:bg-selection-bg",
        disabled && BUTTON_DISABLED,
        props.grow === true && BUTTON_GROW,
      )}
    >
      <Text numberOfLines={1} className={cn(BUTTON_LABEL[size], "text-text-bright")}>
        {props.label}
      </Text>
    </Pressable>
  );
}
