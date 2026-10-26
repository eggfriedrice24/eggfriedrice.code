// The body of the root when no machine is paired (R-NoMachine): a shell line in the blank
// root's voice, "no machine paired" and a prompt with a cursor at 35% that never blinks
// (the shell is there but not ready), then the pairing intro without the modal's nav row.

import { Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { BlockCursor } from "@/components/ui/block-cursor";
import { MonoText } from "@/components/ui/mono-text";
import { SPACING } from "@/theme/metrics";

import { PairingIntro } from "./pairing-intro";

/** The space between the bottom of the bar and the shell line (top 128 on the canvas). */
const SHELL_TOP = 22;

/** The space between the shell line and the intro (the body starts at top 204). */
const INTRO_TOP = 38;

/** The bottom of the footer over the home indicator, as in the modal. */
const FOOTER_BOTTOM = 24;

export interface NoMachineRootProps {
  readonly onScan: () => void;
}

/** The no-machine root, under the transparent bar. */
export function NoMachineRoot(props: NoMachineRootProps) {
  const insets = useSafeAreaInsets();
  return (
    <View
      className="flex-1 bg-surface-1"
      style={{
        paddingTop: insets.top + SPACING.hit + SHELL_TOP,
        paddingBottom: insets.bottom + FOOTER_BOTTOM,
      }}
    >
      <View className="px-gutter">
        <MonoText size="monoShell" tone="text_muted">
          no machine paired
        </MonoText>
        <View className="flex-row items-center" accessibilityElementsHidden>
          <Text className="text-mono-shell text-accent-primary">❯ </Text>
          <BlockCursor size="shell" mode="idle" />
        </View>
      </View>
      <PairingIntro onScan={props.onScan} top={INTRO_TOP} />
    </View>
  );
}
