// The yolk dot of an approval (design sections 2, 3.11 and 3.15): 7 pt on the header
// title and in the strip, 8 pt in an approval block and in a sheet row. It always sits
// next to words that say the same, so colour is never the only signal.

import { View } from "react-native";

import { cn } from "./cn";

/** The size: small (7 pt, header and strip) or large (8 pt, blocks and rows). */
export type DotSize = "small" | "large";

const SIZE_CLASS = {
  small: "w-7 h-7",
  large: "w-8 h-8",
} as const satisfies Record<DotSize, string>;

export interface DotProps {
  readonly size: DotSize;
  /** The VoiceOver label, such as "Needs approval". Without it VoiceOver skips the dot. */
  readonly label?: string;
}

/** A yolk dot. */
export function Dot(props: DotProps) {
  const label = props.label;
  return (
    <View
      testID="dot"
      accessible={label !== undefined}
      accessibilityRole={label === undefined ? undefined : "image"}
      accessibilityLabel={label}
      accessibilityElementsHidden={label === undefined}
      className={cn("rounded-full bg-accent-primary", SIZE_CLASS[props.size])}
    />
  );
}
