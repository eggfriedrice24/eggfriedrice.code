// The frame of the composer (design 3.3): docked to the keyboard with
// KeyboardStickyView, with the chrome material and a hairline on top, 12 pt at the sides
// and the safe area under it (slot E). With the keyboard up, the gap under it is 8 pt;
// the look is the same up or down. The blank root draws no material and no hairline, and
// has 8 pt on top instead of the status line (design item 8.13).

import type { ReactNode } from "react";
import { View, type LayoutChangeEvent } from "react-native";
import { KeyboardStickyView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { cn } from "@/components/ui/cn";
import { Hairline } from "@/components/ui/hairline";
import { Material } from "@/components/ui/material";

import { ComposerContext } from "./composer-context";
import { useComposer, type ComposerOptions } from "./use-composer";

/** The gap between the composer and the keyboard when the keyboard is up. */
export const KEYBOARD_GAP = 8;

/** The bottom edge of the composer: the safe area, and never less than the keyboard gap. */
export function bottomEdge(safeArea: number): number {
  return Math.max(safeArea, KEYBOARD_GAP);
}

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
  const blank = model.isBlankRoot;
  const layout = (event: LayoutChangeEvent): void => {
    onHeightChange?.(event.nativeEvent.layout.height);
  };
  return (
    <ComposerContext value={model}>
      <KeyboardStickyView
        testID="composer"
        offset={{ closed: 0, opened: bottom - KEYBOARD_GAP }}
        onLayout={layout}
      >
        <View className={cn("relative px-12", blank && "pt-8")} style={{ paddingBottom: bottom }}>
          {blank ? null : <Chrome />}
          {children}
        </View>
      </KeyboardStickyView>
    </ComposerContext>
  );
}

/** The material and the top hairline of a docked composer. */
function Chrome() {
  return (
    <>
      <Material kind="chrome" />
      <View className="absolute top-0 right-0 left-0">
        <Hairline />
      </View>
    </>
  );
}
