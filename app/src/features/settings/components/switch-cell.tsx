// A subtitle cell of Settings with a switch: the title, one line under it, and an optional
// second line. The rows "Face ID unlock" and "Notifications" use it.
//
// - The row takes the tap, not the switch: the switch moves only after iOS answered, so
//   it never jumps back. A row that opens the iOS settings is a button with a dim switch.
// - The lines are 13/18 (`text-reason`), in the colour of their tone.
// - VoiceOver reads one element: its label, its role and the hint.

import { Pressable, Switch, Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { TEXT_TONE_CLASS, type TextTone } from "@/components/ui/text-classes";
import { roles } from "@/theme/roles";

/** The colour of a line under the title. */
export type CellTone = Extract<TextTone, "text_subtle" | "text_muted" | "state_warning">;

/** One line under the title. */
export interface CellLine {
  readonly text: string;
  readonly tone: CellTone;
}

export interface SwitchCellProps {
  /** The test ids: `<id>-row`, `<id>-title` and `<id>-line`, and `<id>-note`. */
  readonly id: string;
  readonly title: string;
  readonly line: CellLine;
  /** A second line, such as "Not available on homelab". */
  readonly note?: CellLine | undefined;
  readonly value: boolean;
  /** The title in text_subtle and the switch at 40 %. */
  readonly dim: boolean;
  /** The row takes no tap. */
  readonly disabled: boolean;
  /** iOS checks or asks after a tap. */
  readonly busy?: boolean | undefined;
  readonly onPress: () => void;
  /** `switch`, or `button` for a row that opens the iOS settings. */
  readonly role: "switch" | "button";
  readonly accessibilityLabel: string;
  readonly accessibilityHint: string;
  /** The row is marked with selection_bg. */
  readonly marked?: boolean | undefined;
  readonly onLayout?: (() => void) | undefined;
}

/** The cell with the switch. */
export function SwitchCell(props: SwitchCellProps) {
  const note = props.note;
  return (
    <Pressable
      testID={`${props.id}-row`}
      accessibilityRole={props.role}
      accessibilityLabel={props.accessibilityLabel}
      accessibilityHint={props.accessibilityHint}
      accessibilityState={
        props.role === "switch"
          ? { checked: props.value, busy: props.busy === true, disabled: props.dim }
          : { disabled: props.disabled }
      }
      disabled={props.disabled}
      onPress={props.onPress}
      onLayout={props.onLayout}
      className={cn(
        "min-h-46 flex-row items-center gap-12 px-16 py-10 active:bg-selection-bg",
        props.marked === true && "bg-selection-bg",
      )}
    >
      <View className="flex-1 gap-1">
        <Text
          testID={`${props.id}-title`}
          className={cn("text-row", props.dim ? "text-text-subtle" : "text-text-primary")}
        >
          {props.title}
        </Text>
        <Text
          testID={`${props.id}-line`}
          className={cn("text-reason", TEXT_TONE_CLASS[props.line.tone])}
        >
          {props.line.text}
        </Text>
        {note === undefined ? null : (
          <Text
            testID={`${props.id}-note`}
            className={cn("text-reason", TEXT_TONE_CLASS[note.tone])}
          >
            {note.text}
          </Text>
        )}
      </View>
      <View
        pointerEvents="none"
        accessibilityElementsHidden
        importantForAccessibility="no"
        className={cn(props.dim && "opacity-40")}
      >
        <Switch
          value={props.value}
          disabled={props.dim}
          trackColor={{ true: roles.accent_primary, false: roles.selection_bg }}
          ios_backgroundColor={roles.selection_bg}
        />
      </View>
    </Pressable>
  );
}
