// "Jump to latest" (design 3.12 and the C-Jump board): a 44 pt glass circle with
// arrow.down in text_bright, 16 pt from the right edge and 12 pt above the composer,
// with the shadow of the jump material. iOS 26 draws Liquid Glass; before it, the
// system blur with the glass fill and edge.
//
// It shows when the reader is more than one screen from the end while new content
// arrives, fades in over 150 ms, and hides at the end. A tap scrolls to the end, plays
// the light haptic, and the list follows again. With Reduce Motion the scroll is
// instant and the button does not fade.

import { View } from "react-native";

import { GlassButton } from "@/components/ui/glass-button";
import { haptic } from "@/lib/native/haptics";
import { useReduceMotion } from "@/lib/native/reduce-motion";
import { SPACING } from "@/theme/metrics";
import { MATERIALS } from "@/theme/materials";
import { FadeIn } from "./fade-in";

/** The space between the button and the top of the composer (design item x2). */
export const JUMP_GAP = SPACING.item;

const SHADOW = {
  shadowColor: MATERIALS.jump.shadow.color,
  shadowOpacity: MATERIALS.jump.shadow.opacity,
  shadowRadius: MATERIALS.jump.shadow.radius,
  shadowOffset: { width: 0, height: MATERIALS.jump.shadow.offsetY },
} as const;

export interface JumpToLatestProps {
  readonly visible: boolean;
  /** The height of the composer, from the bottom of the screen. */
  readonly bottom: number;
  /** Scrolls to the end; `animated` is false with Reduce Motion. */
  readonly onJump: (animated: boolean) => void;
}

/** The jump button. */
export function JumpToLatest(props: JumpToLatestProps) {
  const reduceMotion = useReduceMotion();
  if (!props.visible) {
    return null;
  }
  return (
    <View
      pointerEvents="box-none"
      className="absolute right-inset"
      style={{ bottom: props.bottom + JUMP_GAP }}
    >
      <FadeIn fadeOut testID="jump-to-latest">
        <View className="rounded-full" style={SHADOW}>
          <GlassButton
            symbol="arrow.down"
            symbolSize={18}
            label="Jump to latest"
            onPress={() => {
              haptic("light");
              props.onJump(!reduceMotion);
            }}
          />
        </View>
      </FadeIn>
    </View>
  );
}
