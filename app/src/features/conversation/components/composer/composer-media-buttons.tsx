// The two media buttons of the composer, in both sizes of the pill (the user,
// 2026-11-17, as t3code and Claude Code):
//
// - Attach (plus) in slot B, at the bottom left: later it adds a file or an image to
//   the message (the images release, 0.0.13).
// - Voice (mic) before the trailing action: later it records a voice message, which
//   needs a native audio module.
//
// Both are off for now. Each is a 32 pt circle with no fill and a 44 pt hit area, as
// the other actions, with its glyph in text_decorative. VoiceOver reads the button, says
// that it is off and why.

import { Pressable } from "react-native";

import { cn } from "@/components/ui/cn";
import { SymbolIcon, type SymbolName } from "@/components/ui/symbol";
import { SPACING } from "@/theme/metrics";

/** The hit slop that takes a 32 pt action to the 44 pt hit area. */
const SLOP = (SPACING.hit - SPACING.action) / 2;

/** The VoiceOver hint of a media button that is not available yet. */
export const NOT_YET_HINT = "Not available yet.";

interface MediaButtonProps {
  readonly testID: string;
  readonly icon: SymbolName;
  readonly label: string;
}

/** A media button that is off until its feature ships. */
function MediaButton(props: MediaButtonProps) {
  return (
    <Pressable
      testID={props.testID}
      accessibilityRole="button"
      accessibilityLabel={props.label}
      accessibilityHint={NOT_YET_HINT}
      accessibilityState={{ disabled: true }}
      disabled
      hitSlop={SLOP}
      className={cn("w-action h-action my-4 items-center justify-center rounded-full")}
    >
      <SymbolIcon name={props.icon} size={17} weight="medium" tone="text_decorative" />
    </Pressable>
  );
}

/** Slot B: attach a file or an image (off for now). */
export function ComposerAttachButton() {
  return <MediaButton testID="composer-attach" icon="plus" label="Attach a file or image" />;
}

/** Before the trailing action: record a voice message (off for now). */
export function ComposerVoiceButton() {
  return <MediaButton testID="composer-voice" icon="mic" label="Record a voice message" />;
}
