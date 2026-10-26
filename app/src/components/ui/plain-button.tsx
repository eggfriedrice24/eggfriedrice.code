// A plain text action in mono 11 accent_ui (UIButton.Configuration.plain() in the
// design): "retry" and "copy error" of a failed turn, "withdraw" of a queued message,
// "show all 14 lines", "show full diff" and "collapse". The design draws it 32 pt high
// (24 pt for withdraw); the hit area reaches 44 pt.

import { Pressable, Text } from "react-native";

import { SPACING } from "@/theme/metrics";

import { cn } from "./cn";

/**
 * The height: `row` is a 32 pt button across a block (show all, show full diff), and
 * `inline` is a 24 pt word in a line of text (withdraw, retry, copy error).
 */
export type PlainButtonHeight = "row" | "inline";

const HEIGHT = {
  row: { className: "h-32 px-12", height: 32 },
  inline: { className: "min-h-24", height: 24 },
} as const satisfies Record<PlainButtonHeight, { className: string; height: number }>;

export interface PlainButtonProps {
  readonly label: string;
  readonly onPress: () => void;
  readonly height?: PlainButtonHeight;
  /** True for a button that spans its block and puts the label at the start. */
  readonly wide?: boolean;
  readonly disabled?: boolean;
  /** The VoiceOver label, when the visible words are not enough. */
  readonly accessibilityLabel?: string;
}

/** A plain text action. */
export function PlainButton(props: PlainButtonProps) {
  const height = HEIGHT[props.height ?? "inline"];
  const slop = Math.max(0, (SPACING.hit - height.height) / 2);
  const disabled = props.disabled === true;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.accessibilityLabel ?? props.label}
      accessibilityState={{ disabled }}
      disabled={disabled}
      hitSlop={{ top: slop, bottom: slop }}
      onPress={props.onPress}
      className={cn(
        "justify-center active:opacity-60",
        height.className,
        props.wide === true && "self-stretch",
        disabled && "opacity-40",
      )}
    >
      <Text numberOfLines={1} className="text-mono-xs text-accent-ui">
        {props.label}
      </Text>
    </Pressable>
  );
}
