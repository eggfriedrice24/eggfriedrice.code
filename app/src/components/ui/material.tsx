// The materials of our own views (design section 0 and note 10). The app uses the system
// blur, never a painted fill, where the canvas draws a role at an opacity:
//
// - `chrome`: the strip under the bar and the docked composer, the same material as the
//   native bar (systemChromeMaterialDark through expo-blur).
// - `glass`: a glass button or group and the jump button. iOS 26 draws Liquid Glass
//   (expo-glass-effect). Before iOS 26 it is the system blur with the canvas fill and the
//   1 pt border_subtle edge (theme/materials.ts).
//
// The material fills its parent: put it first in a view with `overflow-hidden` and the
// corner radius of the shape. It takes no touches.
//
// NOTE: BlurView and GlassView are views of packages, which take no uniwind class
// without a wrapper, so they get a style. uniwind ships TypeScript source, which the
// jest-expo preset does not transform, so a component that the tests render does not
// import "uniwind".

import { BlurView } from "expo-blur";
import { GlassView, isLiquidGlassAvailable } from "expo-glass-effect";
import { StyleSheet, View } from "react-native";

import { MATERIALS } from "@/theme/materials";

/** The kind of material. */
export type MaterialKind = "chrome" | "glass";

/** True on iOS 26 and later, where glass is Liquid Glass. It does not change while the app runs. */
export const LIQUID_GLASS = isLiquidGlassAvailable();

export interface MaterialProps {
  readonly kind: MaterialKind;
  /** The corner radius of the shape, so the glass and the edge follow it. */
  readonly radius?: number;
}

/** A material that fills its parent. */
export function Material(props: MaterialProps) {
  const radius = props.radius ?? 0;
  if (props.kind === "chrome") {
    return (
      <BlurView
        testID="material-chrome"
        pointerEvents="none"
        tint={MATERIALS.chrome.blurTint}
        intensity={MATERIALS.chrome.blurIntensity}
        style={[StyleSheet.absoluteFill, { borderRadius: radius }]}
      />
    );
  }
  if (LIQUID_GLASS) {
    return (
      <GlassView
        testID="material-glass"
        pointerEvents="none"
        glassEffectStyle="regular"
        colorScheme="dark"
        style={[StyleSheet.absoluteFill, { borderRadius: radius }]}
      />
    );
  }
  return (
    <View testID="material-glass" pointerEvents="none" className="absolute inset-0">
      <BlurView
        tint={MATERIALS.chrome.blurTint}
        intensity={MATERIALS.chrome.blurIntensity}
        style={[StyleSheet.absoluteFill, { borderRadius: radius }]}
      />
      <View
        className="absolute inset-0"
        style={{
          borderRadius: radius,
          backgroundColor: MATERIALS.glassButton.fill,
          borderColor: MATERIALS.glassButton.borderColor,
          borderWidth: MATERIALS.glassButton.borderWidth,
        }}
      />
    </View>
  );
}
