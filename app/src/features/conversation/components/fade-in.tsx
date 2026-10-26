// The short fade of new things in the transcript (design section 4): a new line of tool
// output and the "Jump to latest" button fade in over about 150 ms. It is a Reanimated
// layout animation, so it runs on the UI thread and needs no effect. With Reduce Motion
// the view appears at once.
//
// This is the one file of the transcript that imports Reanimated, so the component
// tests replace it with a plain view.

import type { ReactNode } from "react";
import { View } from "react-native";
import Animated, { FadeIn as FadeInAnimation, FadeOut } from "react-native-reanimated";

import { useReduceMotion } from "@/lib/native/reduce-motion";

/** The time of the fade, in ms (design: about 150 ms). */
export const FADE_MS = 150;

export interface FadeInProps {
  readonly children: ReactNode;
  /** True to fade out too when the view goes, such as the jump button. */
  readonly fadeOut?: boolean;
  readonly testID?: string;
}

/** A view that fades in when it mounts. */
export function FadeIn(props: FadeInProps) {
  const reduceMotion = useReduceMotion();
  if (reduceMotion) {
    return <View testID={props.testID}>{props.children}</View>;
  }
  if (props.fadeOut === true) {
    return (
      <Animated.View
        testID={props.testID}
        entering={FadeInAnimation.duration(FADE_MS)}
        exiting={FadeOut.duration(FADE_MS)}
      >
        {props.children}
      </Animated.View>
    );
  }
  return (
    <Animated.View testID={props.testID} entering={FadeInAnimation.duration(FADE_MS)}>
      {props.children}
    </Animated.View>
  );
}
