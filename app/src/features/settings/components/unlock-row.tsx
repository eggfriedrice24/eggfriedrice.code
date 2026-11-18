// The row "Face ID unlock" of Settings (Settings round 9, frames 1 to 6), in the group
// Privacy. A subtitle cell with a switch: the title follows the iPhone ("Face ID unlock",
// "Touch ID unlock", "Passcode unlock"), and one line under it says what it does.
//
// - The switch moves only after the check of iOS passed, so it never jumps back (frame
//   3). While iOS checks, the row takes no tap.
// - An iPhone with no passcode greys the title (text_subtle) and the switch, and says
//   "Needs a passcode on this iPhone." (frame 5). After efr turned the unlock off for
//   that reason, the line says so in text_muted (frame 6) while the iPhone has no
//   passcode. The line stays readable: it is the only text that tells why.
// - VoiceOver reads the switch, its name, and the line as its hint.

import { SwitchCell } from "./switch-cell";

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
  /** The row is marked with selection_bg (Settings opened with `focus=unlock`). */
  readonly marked?: boolean;
  readonly onLayout?: () => void;
}

/** The row with the switch. */
export function UnlockRow(props: UnlockRowProps) {
  return (
    <SwitchCell
      id="unlock"
      title={props.title}
      line={{ text: props.line, tone: props.notice === true ? "text_muted" : "text_subtle" }}
      value={props.value}
      dim={props.disabled}
      disabled={props.disabled || props.checking}
      busy={props.checking}
      onPress={() => props.onValueChange(!props.value)}
      role="switch"
      accessibilityLabel={props.title}
      accessibilityHint={props.line}
      marked={props.marked}
      onLayout={props.onLayout}
    />
  );
}
