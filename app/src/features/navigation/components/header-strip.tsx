// The strip under the bar (design section 2, items 8.6 and 8.19), as t3code floats its
// controls: a 44 pt glass capsule 8 pt under the transparent bar and 12 pt from the
// sides of the screen. Mono 11, text_muted. iPhone has no public accessory of the
// navigation bar, so the strip is a view of ours (design note 7). On iOS 26 the glass is
// Liquid Glass; before it, the system blur with the glass fill and edge.

import { Pressable, View } from "react-native";

import { Dot } from "@/components/ui/dot";
import { Material } from "@/components/ui/material";
import { MonoText } from "@/components/ui/mono-text";
import { SymbolIcon } from "@/components/ui/symbol";
import { SPACING } from "@/theme/metrics";
import { slotLabel, type SlotContent } from "../model/header-slot";

export interface HeaderStripProps {
  readonly content: SlotContent;
  /** The distance from the top of the screen to the bottom of the bar. */
  readonly top: number;
  /** A tap on an approval strip. The offline line takes no tap. */
  readonly onPress: () => void;
}

/** The strip, or nothing when the slot is empty. */
export function HeaderStrip(props: HeaderStripProps) {
  const { content } = props;
  if (content.kind === "none") {
    return null;
  }
  return (
    <View
      testID="header-strip"
      className="absolute right-12 left-12 h-strip overflow-hidden rounded-full"
      style={{ top: props.top + SPACING.float }}
    >
      <Material kind="glass" radius={SPACING.strip / 2} />
      {content.kind === "offline" ? (
        <View
          accessible
          accessibilityRole="text"
          accessibilityLiveRegion="polite"
          accessibilityLabel={slotLabel(content)}
          role="status"
          className="flex-1 flex-row items-center gap-8 px-16"
        >
          <MonoText numberOfLines={1} tone="text_subtle" className="shrink">
            <MonoText tone="text_primary">{content.machineName}</MonoText>
            {` ${content.words}`}
          </MonoText>
        </View>
      ) : (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={slotLabel(content)}
          onPress={props.onPress}
          className="flex-1 flex-row items-center gap-8 px-16"
        >
          <Dot size="small" />
          {content.kind === "approval" ? (
            <MonoText numberOfLines={1} tone="text_muted" className="flex-1">
              <MonoText tone="accent_primary">Approval waiting</MonoText>
              {" · "}
              <MonoText tone="text_primary">{content.machineName}</MonoText>
              {" · "}
              <MonoText tone="text_primary">{content.title}</MonoText>
            </MonoText>
          ) : (
            <MonoText numberOfLines={1} tone="accent_primary" className="flex-1">
              {content.text}
            </MonoText>
          )}
          <SymbolIcon name="chevron.right" size={10} tone="text_line_number" weight="heavy" />
        </Pressable>
      )}
    </View>
  );
}
