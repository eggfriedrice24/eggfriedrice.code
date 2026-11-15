// The row "Face ID unlock" of Settings (Settings round 9, frames 1 to 6), in the group
// Privacy. A subtitle cell with a switch: the title follows the iPhone ("Face ID unlock",
// "Touch ID unlock", "Passcode unlock"), and one line under it says what it does.
//
// - The row takes the tap, not the switch: the switch moves only after the check of iOS
//   passed, so it never jumps back (frame 3). While iOS checks, the row takes no tap.
// - An iPhone with no passcode greys the title (text_subtle) and the switch, and says
//   "Needs a passcode on this iPhone." (frame 5). After efr turned the unlock off for
//   that reason, the line says so in text_muted (frame 6) while the iPhone has no
//   passcode. The line stays readable: it is the only text that tells why.
// - The line is 13/18 in text_subtle (the board).
// - VoiceOver reads one element: the switch, its name, and the line as its hint.

import { Pressable, Switch, Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { roles } from "@/theme/roles";

export interface UnlockRowProps {
  /** `unlockName(method)`. */
  readonly title: string;
  /** `unlockRowLine(facts)`, or the notice of D5. */
  readonly line: string;
  /** The line is the notice of frame 6. */
  readonly notice?: boolean;
  readonly value: boolean;
  /** iOS checks after a tap; the switch did not move. */
  readonly checking: boolean;
  /** No passcode on this iPhone, and the switch is off. */
  readonly disabled: boolean;
  /** A tap on the row: the switch should go to `on`. */
  readonly onValueChange: (on: boolean) => void;
}

/** The row with the switch. */
export function UnlockRow(props: UnlockRowProps) {
  const off = props.disabled || props.checking;
  return (
    <Pressable
      testID="unlock-row"
      accessibilityRole="switch"
      accessibilityLabel={props.title}
      accessibilityHint={props.line}
      accessibilityState={{ checked: props.value, busy: props.checking, disabled: props.disabled }}
      disabled={off}
      onPress={() => props.onValueChange(!props.value)}
      className="min-h-46 flex-row items-center gap-12 px-16 py-10 active:bg-selection-bg"
    >
      <View className="flex-1 gap-1">
        <Text
          testID="unlock-title"
          className={cn("text-row", props.disabled ? "text-text-subtle" : "text-text-primary")}
        >
          {props.title}
        </Text>
        <Text
          testID="unlock-line"
          className={cn(
            "text-reason",
            props.notice === true ? "text-text-muted" : "text-text-subtle",
          )}
        >
          {props.line}
        </Text>
      </View>
      <View
        pointerEvents="none"
        accessibilityElementsHidden
        importantForAccessibility="no"
        className={cn(props.disabled && "opacity-40")}
      >
        <Switch
          value={props.value}
          disabled={props.disabled}
          trackColor={{ true: roles.accent_primary, false: roles.selection_bg }}
          ios_backgroundColor={roles.selection_bg}
        />
      </View>
    </Pressable>
  );
}
