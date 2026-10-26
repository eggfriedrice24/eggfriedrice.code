// The frame of the composer (design 3.3, as t3code draws it): a glass pill that floats
// over a scrim, 12 pt from the sides of the screen, docked to the keyboard with
// KeyboardStickyView.
//
// - The pill has radius 24 and clips its content. The glass is its first child, then the
//   status line and the row, so the field needs no border of its own.
// - The scrim behind it is surface_1 that goes from 0 to 0.6 to 0.9 opacity, so the
//   transcript fades out under the composer. It is not a blur.
// - Under the pill is the bottom edge: the safe area, but never less than 12 pt, then a
//   gap of 6 pt. With the keyboard up, the sticky view moves the composer down by the
//   bottom edge, so only the gap of 6 pt stays over the keyboard.
// - The blank root has the same pill and the same status line. The user chose this over
//   design item 8.13, which had no status line and no material there.

import type { ReactNode } from "react";
import { View, type LayoutChangeEvent } from "react-native";
import { KeyboardStickyView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { Material } from "@/components/ui/material";
import { RADII } from "@/theme/metrics";

import { ComposerContext } from "./composer-context";
import { useComposer, type ComposerOptions } from "./use-composer";

/** The gap between the pill and the keyboard, or between the pill and the bottom edge. */
export const KEYBOARD_GAP = 6;

/** The smallest bottom edge, on a screen without a home indicator. */
export const MIN_BOTTOM_EDGE = 12;

/** The bottom edge of the composer: the safe area, and never less than 12 pt. */
export function bottomEdge(safeArea: number): number {
  return Math.max(safeArea, MIN_BOTTOM_EDGE);
}

/** The scrim behind the pill: surface_1 at 0, then 0.6, then 0.9 opacity, top to bottom. */
export const SCRIM_CLASS =
  "absolute inset-0 bg-linear-to-b from-surface-1/0 via-surface-1/60 to-surface-1/90";

export interface ComposerRootProps extends ComposerOptions {
  /**
   * Called with the height of the composer when it changes, so the screen can keep the
   * transcript above it and put "Jump to latest" 12 pt above its top (design item x2).
   */
  readonly onHeightChange?: (height: number) => void;
  /** The slots: StatusLine, then Row. */
  readonly children: ReactNode;
}

/** The composer: it gives its model to the slots. */
export function ComposerRoot(props: ComposerRootProps) {
  const { children, onHeightChange, ...options } = props;
  const model = useComposer(options);
  const bottom = bottomEdge(useSafeAreaInsets().bottom);
  const layout = (event: LayoutChangeEvent): void => {
    onHeightChange?.(event.nativeEvent.layout.height);
  };
  return (
    <ComposerContext value={model}>
      <KeyboardStickyView
        testID="composer"
        offset={{ closed: 0, opened: bottom }}
        onLayout={layout}
      >
        <View className="px-12 pt-6" style={{ paddingBottom: bottom + KEYBOARD_GAP }}>
          <View testID="composer-scrim" pointerEvents="none" className={SCRIM_CLASS} />
          <View testID="composer-pill" className="overflow-hidden rounded-composer">
            <Material kind="glass" radius={RADII.composer} />
            {children}
          </View>
        </View>
      </KeyboardStickyView>
    </ComposerContext>
  );
}
