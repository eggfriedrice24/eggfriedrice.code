// The native UISlider of the effort (@react-native-community/slider). It needs a dev build
// that has the native module. A build of the app from before the module has no RNCSlider
// view, so the sheet then draws the steps as a row of buttons instead (S-ModelSheet still
// works on such a build).

import type { ComponentType } from "react";
import { type StyleProp, UIManager, type ViewStyle } from "react-native";

/** The props of the slider that the sheet uses. */
export interface NativeSliderProps {
  readonly minimumValue: number;
  readonly maximumValue: number;
  readonly step: number;
  readonly value: number;
  readonly disabled?: boolean;
  readonly minimumTrackTintColor: string;
  readonly maximumTrackTintColor: string;
  readonly thumbTintColor?: string;
  /** Each stop that the thumb reaches while the finger moves. */
  readonly onValueChange?: (value: number) => void;
  readonly onSlidingComplete: (value: number) => void;
  readonly accessibilityLabel?: string;
  readonly accessibilityIncrements?: string[];
  readonly testID?: string;
  readonly style?: StyleProp<ViewStyle>;
}

/** The slider of the build, or undefined when the build has no native module for it. */
function load(): ComponentType<NativeSliderProps> | undefined {
  try {
    if (!UIManager.hasViewManagerConfig("RNCSlider")) {
      return undefined;
    }
    // The require runs only when the build has the view, so an old build never asks for
    // the view config of a view that it does not have.
    const slider: {
      readonly default: ComponentType<NativeSliderProps>;
    } = require("@react-native-community/slider");
    return slider.default;
  } catch {
    return undefined;
  }
}

/** The native slider, or undefined in a build without it (and in the tests). */
export const NATIVE_SLIDER: ComponentType<NativeSliderProps> | undefined = load();
