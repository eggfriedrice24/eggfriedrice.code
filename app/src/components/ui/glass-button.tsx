// Glass buttons (design section 2 and 3.12): a 44 pt glass circle with one SF Symbol in
// text_bright, such as the close button of pairing and "Jump to latest", and a glass group
// of 46 x 44 items. The fill turns to selection_bg while the button is pressed or while
// its menu is open. The header buttons of the native bar are native (Stack.Toolbar), not
// these.

import type { ReactNode } from "react";
import { Pressable, View } from "react-native";

import { SPACING } from "@/theme/metrics";

import { cn } from "./cn";
import { Material } from "./material";
import { SymbolIcon, type SymbolName } from "./symbol";

const CIRCLE_RADIUS = SPACING.hit / 2;

export interface GlassButtonProps {
  readonly symbol: SymbolName;
  /** The size of the symbol in points (for example 18 for arrow.down, 15 for xmark). */
  readonly symbolSize: number;
  /** The VoiceOver label, such as "Close" or "Jump to latest". */
  readonly label: string;
  readonly onPress: () => void;
  /** True while the menu of the button is open: the fill is selection_bg. */
  readonly open?: boolean;
}

/** A 44 pt glass circle with one symbol. */
export function GlassButton(props: GlassButtonProps) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.label}
      accessibilityState={{ expanded: props.open }}
      onPress={props.onPress}
      className="w-hit h-hit items-center justify-center overflow-hidden rounded-full"
    >
      {({ pressed }) => (
        <>
          <Material kind="glass" radius={CIRCLE_RADIUS} />
          <PressedFill shown={pressed || props.open === true} />
          <SymbolIcon
            name={props.symbol}
            size={props.symbolSize}
            tone="text_bright"
            weight="semibold"
          />
        </>
      )}
    </Pressable>
  );
}

export interface GlassGroupProps {
  /** The items: GlassGroupItem elements. */
  readonly children: ReactNode;
}

/** A glass group of buttons, 44 pt high, such as Settings and New. */
export function GlassGroup(props: GlassGroupProps) {
  return (
    <View className="h-hit flex-row overflow-hidden rounded-full">
      <Material kind="glass" radius={CIRCLE_RADIUS} />
      {props.children}
    </View>
  );
}

export interface GlassGroupItemProps {
  readonly symbol: SymbolName;
  readonly symbolSize: number;
  readonly label: string;
  readonly onPress: () => void;
}

/** One 46 x 44 item of a glass group. */
export function GlassGroupItem(props: GlassGroupItemProps) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.label}
      onPress={props.onPress}
      className="w-46 h-hit items-center justify-center"
    >
      {({ pressed }) => (
        <>
          <PressedFill shown={pressed} />
          <SymbolIcon name={props.symbol} size={props.symbolSize} tone="text_bright" />
        </>
      )}
    </Pressable>
  );
}

function PressedFill(props: { readonly shown: boolean }) {
  return <View className={cn("absolute inset-0", props.shown && "bg-selection-bg")} />;
}
