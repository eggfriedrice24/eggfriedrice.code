// The layout of the pairing screens (design 3.17): the modal's own nav row (close and
// title), the body at left and right 24, 44 pt under the nav row, and the footer at left
// and right 24, 24 pt over the home indicator, with a gap of 12.

import type { ReactNode } from "react";
import { ScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { BodyText } from "@/components/ui/body-text";
import { cn } from "@/components/ui/cn";
import { GlassButton } from "@/components/ui/glass-button";
import { MonoText } from "@/components/ui/mono-text";
import { SPACING } from "@/theme/metrics";

/** The space between the footer and the home indicator. */
const FOOTER_BOTTOM = 24;

export interface PairingModalFrameProps {
  /** The nav row (PairingNavRow). */
  readonly navRow: ReactNode;
  /** The body and the footer. */
  readonly children: ReactNode;
}

/** The full screen of the pairing modal, from the status bar to the home indicator. */
export function PairingModalFrame(props: PairingModalFrameProps) {
  const insets = useSafeAreaInsets();
  return (
    <View
      className="flex-1 bg-surface-1"
      style={{ paddingTop: insets.top, paddingBottom: insets.bottom + FOOTER_BOTTOM }}
    >
      {props.navRow}
      {props.children}
    </View>
  );
}

export interface PairingNavRowProps {
  /** "Pair a machine", or "Re-pair laptop". */
  readonly title: string;
  readonly onClose: () => void;
  /** True while the system scanner covers the screen: the row is at 60%. */
  readonly dimmed?: boolean;
}

/**
 * The nav row of the modal: the glass close button, the title, and a 44 pt space on the
 * right, so that the title is in the centre.
 */
export function PairingNavRow(props: PairingNavRowProps) {
  return (
    <View
      testID="pairing-nav-row"
      className={cn(
        "h-hit flex-row items-center gap-8 px-inset",
        props.dimmed === true && "opacity-60",
      )}
    >
      <GlassButton symbol="xmark" symbolSize={15} label="Close" onPress={props.onClose} />
      <BodyText
        size="navTitle"
        tone="text_bright"
        numberOfLines={1}
        accessibilityRole="header"
        className="flex-1 text-center"
      >
        {props.title}
      </BodyText>
      <View className="w-hit" />
    </View>
  );
}

/** The gap between the parts of a body: 16 for the steps, 10 for errors, 6 for success. */
export type BodyGap = "step" | "error" | "success";

const GAP = {
  step: "gap-16",
  error: "gap-10",
  success: "gap-6",
} as const satisfies Record<BodyGap, string>;

export interface PairingBodyProps {
  readonly gap: BodyGap;
  readonly children: ReactNode;
  /**
   * The space above the body. In the modal it is 44 pt under the nav row; on the
   * no-machine root the shell line comes first.
   */
  readonly top?: number | undefined;
}

/** The body of a pairing screen. It scrolls when Dynamic Type makes it tall. */
export function PairingBody(props: PairingBodyProps) {
  return (
    <ScrollView
      className="flex-1"
      contentContainerClassName={cn("px-pairing", GAP[props.gap])}
      contentContainerStyle={{ paddingTop: props.top ?? SPACING.hit }}
      alwaysBounceVertical={false}
    >
      {props.children}
    </ScrollView>
  );
}

export interface PairingFooterProps {
  /** The hint above the action, such as "next: point the camera at the code in the terminal". */
  readonly hint?: string;
  readonly children: ReactNode;
}

/** The footer of a pairing screen: an optional hint and one action. */
export function PairingFooter(props: PairingFooterProps) {
  const hint = props.hint;
  return (
    <View className="gap-12 px-pairing">
      {hint === undefined ? null : <MonoText className="text-center">{hint}</MonoText>}
      {props.children}
    </View>
  );
}
