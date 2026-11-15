// The one effort slider of the model sheet (S-ModelSheet "Effort"): a native UISlider with
// a stop per effort of the selected model, the track in selection_bg, the part before the
// thumb in yolk, and the step labels under it. A 4 pt dot under a label marks the
// model's default. The label of the step in use is text_bright.
//
// - The stops are the efforts of the model in `models.list`, so a model without `xhigh`
//   has no `xhigh` stop, and a model without efforts has no slider.
// - The thumb moves from stop to stop only (the patch of @react-native-community/slider
//   snaps it while the finger moves), and a light haptic marks each stop.
// - Each label stands in a box of the same width, centred under its stop, so the bold
//   label in use moves no other label, and the row keeps one height: nothing above or
//   below the slider shifts while it moves.
//
// A build without the native module (native/slider.ts) draws the labels as a row of
// buttons instead, so the sheet works before the next dev build. A tap on a label picks
// that step in both forms.

import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { haptic } from "@/lib/native/haptics";
import { roles } from "@/theme/roles";
import type { EffortView } from "../model/model-sheet";
import { NATIVE_SLIDER } from "../native/slider";

// The native slider is not a view of uniwind, so `className` does not reach it.
const STYLES = StyleSheet.create({ slider: { height: 28 } });

/** The centre of the thumb of UISlider at either end, from the edge of the track. */
const THUMB_HALF = 14;

/** The width of the box of one label. */
const LABEL_WIDTH = 56;

/** The left edge of the box of label `index` of `count` on a track of `width`. */
export function labelLeft(index: number, count: number, width: number): number {
  if (count < 2) {
    return width / 2 - LABEL_WIDTH / 2;
  }
  const span = width - 2 * THUMB_HALF;
  return THUMB_HALF + (span * index) / (count - 1) - LABEL_WIDTH / 2;
}

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
  // Short local UI state: the width of the track, and the stop under the finger.
  const [width, setWidth] = useState(0);
  const [live, setLive] = useState<number | undefined>(undefined);
  const pick = (index: number): void => {
    setLive(undefined);
    const step = effort.steps[Math.round(index)];
    if (step !== undefined) {
      onChange(step);
    }
  };
  const last = effort.steps.length - 1;
  const shown = live ?? effort.index;
  return (
    <View testID="effort-slider" onLayout={(event) => setWidth(event.nativeEvent.layout.width)}>
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
          onValueChange={(value) => {
            const stop = Math.round(value);
            if (stop !== shown) {
              setLive(stop);
              haptic("light");
            }
          }}
          onSlidingComplete={pick}
          accessibilityLabel="Effort"
          accessibilityIncrements={[...effort.steps]}
          style={STYLES.slider}
        />
      )}
      <View className="h-24">
        {effort.steps.map((step, index) => {
          const on = index === shown;
          const isDefault = index === effort.defaultIndex;
          return (
            <Pressable
              key={step}
              accessibilityRole="button"
              accessibilityLabel={`${step} effort${isDefault ? ", default" : ""}`}
              accessibilityState={{ selected: on }}
              hitSlop={{ top: 10, bottom: 10 }}
              onPress={() => pick(index)}
              className="absolute top-0 h-24 items-center active:opacity-60"
              style={{
                left: labelLeft(index, effort.steps.length, width),
                width: LABEL_WIDTH,
              }}
            >
              <Text
                numberOfLines={1}
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
