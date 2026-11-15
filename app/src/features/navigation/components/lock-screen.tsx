// The lock screen and the cover of the Face ID unlock (S-Lock, round 9). Both draw props
// only, so the gallery shows them too; lock-overlay.tsx gives them the lock.
//
// - The lock screen is surface_1 with the efr mark (the egg, the yolk and the ❯), 76 pt, a
//   little above the centre. It shows nothing of a conversation, a machine or a path.
//   After a check that did not pass, one line under the mark says why, centred, and wraps
//   at large sizes. Unlock is the one action, filled, at the bottom right, at the size and
//   place of Allow in the approval sheet.
// - Unlock stays there while iOS asks too: the prompt of iOS lies over it, and after a
//   prompt that never answered it is the way out (the lock replaces such a check after 3
//   s).
// - The cover is surface_1 and the mark at the same place, with no text, so the step from
//   the cover to the lock screen is calm. It has no element for VoiceOver.
// - They show and go with no animation: the cover must be there in one frame.

import { Image, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { SmallFilledButton } from "@/components/ui/small-buttons";

// Metro resolves the image by its path; an import would not type without a module
// declaration for PNG files.
const MARK: number = require("../../../../assets/icon/mark.png");

/** The space under Unlock on a screen without a home indicator. */
const MIN_BOTTOM = 12;

/** The mark, a little above the centre of the window. */
function Mark(props: { readonly label?: string }) {
  const accessible = props.label !== undefined;
  return (
    <Image
      testID="lock-mark"
      source={MARK}
      accessible={accessible}
      accessibilityRole={accessible ? "header" : "none"}
      accessibilityLabel={props.label}
      className="h-76 w-76"
    />
  );
}

export interface LockScreenProps {
  /** The line of the last check that did not pass, or none. */
  readonly line: string | undefined;
  /** The VoiceOver hint of Unlock: "Asks for Face ID." */
  readonly hint: string;
  readonly onUnlock: () => void;
}

/** The lock screen: the mark, the line, and Unlock. */
export function LockScreen(props: LockScreenProps) {
  const bottom = Math.max(useSafeAreaInsets().bottom, MIN_BOTTOM);
  return (
    <View testID="lock-screen" className="flex-1 bg-surface-1">
      <View className="flex-1 items-center justify-center gap-16 px-32 pb-80">
        <Mark label="efr is locked." />
        {props.line === undefined ? null : (
          <Text testID="lock-line" className="text-center text-reason text-text-muted">
            {props.line}
          </Text>
        )}
      </View>
      <View className="flex-row justify-end px-20" style={{ paddingBottom: bottom + 8 }}>
        <SmallFilledButton label="Unlock" accessibilityHint={props.hint} onPress={props.onUnlock} />
      </View>
    </View>
  );
}

/** The cover of the app switcher: the mark only. */
export function LockCover() {
  return (
    <View
      testID="lock-cover"
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      className="flex-1 items-center justify-center bg-surface-1 pb-80"
    >
      <Mark />
    </View>
  );
}
