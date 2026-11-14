// The small buttons of the approval strip and the approval sheet (boards S-Approvals,
// C-Waiting and AP-Sheets): only the primary action is filled. Allow and Next are a yolk
// capsule (`.small .capsule .filled`); Deny and Close are a plain word (`.small .plain`) in
// text_bright. Both are 28 pt high, and the touch reaches 44 pt without a neighbour's.
// While its action runs, a button shows the braille cell in place of its label.

import { Pressable, Text } from "react-native";

import { BrailleCell } from "./braille-cell";
import { cn } from "./cn";

/** How far the touch reaches above and under the 28 pt button: 44 pt in all. */
const SLOP = { top: 8, bottom: 8 } as const;

export interface SmallButtonProps {
  readonly label: string;
  readonly onPress: () => void;
  /** True while the action runs: the braille cell replaces the label. */
  readonly pending?: boolean;
  /** The VoiceOver label while pending, such as "Allowing". */
  readonly pendingLabel?: string;
  readonly disabled?: boolean;
}

/** The filled yolk capsule: Allow, Next. */
export function SmallFilledButton(props: SmallButtonProps) {
  const pending = props.pending === true;
  const disabled = props.disabled === true;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={pending ? (props.pendingLabel ?? props.label) : props.label}
      accessibilityState={{ disabled: disabled || pending, busy: pending }}
      disabled={disabled || pending}
      hitSlop={SLOP}
      onPress={props.onPress}
      className={cn(
        "h-28 min-w-58 flex-row items-center justify-center rounded-full bg-accent-primary px-14 active:opacity-80",
        disabled && !pending && "opacity-40",
      )}
    >
      {pending ? (
        <BrailleCell size="button" tint="on_accent" />
      ) : (
        <Text numberOfLines={1} className="text-button-small text-text-on-accent">
          {props.label}
        </Text>
      )}
    </Pressable>
  );
}

/** The plain word: Deny, Close. */
export function SmallPlainButton(props: SmallButtonProps) {
  const pending = props.pending === true;
  const disabled = props.disabled === true;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={pending ? (props.pendingLabel ?? props.label) : props.label}
      accessibilityState={{ disabled: disabled || pending, busy: pending }}
      disabled={disabled || pending}
      hitSlop={SLOP}
      onPress={props.onPress}
      className={cn(
        "h-28 flex-row items-center justify-center rounded-full px-10 active:opacity-60",
        disabled && !pending && "opacity-40",
      )}
    >
      {pending ? (
        <BrailleCell size="button" />
      ) : (
        <Text numberOfLines={1} className="text-button-small text-text-bright">
          {props.label}
        </Text>
      )}
    </Pressable>
  );
}
