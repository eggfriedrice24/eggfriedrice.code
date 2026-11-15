// The one effort slider of the model sheet (S-ModelSheet "Effort"): a native UISlider with
// a step per effort of the selected model, the track in selection_bg, the part before the
// thumb in yolk, and the step labels under it. A 4 pt dot under a label marks the
// model's default. The label of the step in use is text_bright.
//
// A build without the native module (native/slider.ts) draws the labels as a row of
// buttons instead, so the sheet works before the next dev build. A tap on a label picks
// that step in both forms.

import { Pressable, StyleSheet, Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { roles } from "@/theme/roles";
import type { EffortView } from "../model/model-sheet";
import { NATIVE_SLIDER } from "../native/slider";

// The native slider is not a view of uniwind, so `className` does not reach it.
const STYLES = StyleSheet.create({ slider: { height: 28 } });

export interface EffortSliderProps {
  readonly effort: EffortView;
  /** Picks `effort`, one of the steps, after a slide or a tap on its label. */
  readonly onChange: (effort: string) => void;
  /** True to draw the row of buttons, as a build without the native slider does. */
  readonly fallback?: boolean;
}

/** The effort slider, or its row of buttons. */
export function EffortSlider(props: EffortSliderProps) {
  const { effort, onChange } = props;
  const Slider = props.fallback === true ? undefined : NATIVE_SLIDER;
  const pick = (index: number): void => {
    const step = effort.steps[Math.round(index)];
    if (step !== undefined) {
      onChange(step);
    }
  };
  const last = effort.steps.length - 1;
  return (
    <View testID="effort-slider">
      {Slider === undefined || last < 1 ? null : (
        <Slider
          testID="effort-native-slider"
          minimumValue={0}
          maximumValue={last}
          step={1}
          value={effort.index}
          minimumTrackTintColor={roles.accent_primary}
          maximumTrackTintColor={roles.selection_bg}
          thumbTintColor={roles.text_bright}
          onSlidingComplete={pick}
          accessibilityLabel="Effort"
          accessibilityIncrements={[...effort.steps]}
          style={STYLES.slider}
        />
      )}
      <View className="flex-row justify-between">
        {effort.steps.map((step, index) => {
          const on = index === effort.index;
          const isDefault = index === effort.defaultIndex;
          return (
            <Pressable
              key={step}
              accessibilityRole="button"
              accessibilityLabel={`${step} effort${isDefault ? ", default" : ""}`}
              accessibilityState={{ selected: on }}
              hitSlop={{ top: 10, bottom: 10 }}
              onPress={() => pick(index)}
              className="min-h-24 items-center active:opacity-60"
            >
              <Text
                className={
                  on ? "text-caption-strong text-text-bright" : "text-caption text-text-subtle"
                }
              >
                {step}
              </Text>
              {isDefault ? (
                <View
                  testID="effort-default-dot"
                  className={cn(
                    "mt-3 h-4 w-4 rounded-full",
                    on ? "bg-text-bright" : "bg-text-muted",
                  )}
                />
              ) : null}
            </Pressable>
          );
        })}
      </View>
    </View>
  );
}
