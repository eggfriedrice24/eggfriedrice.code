// A 0.5 pt hairline in border_subtle: separators, the line under a code block header,
// the edges of the strip and the composer (design section 0: every hairline of the mock
// is #46516c).

import { View } from "react-native";

import { cn } from "./cn";

/**
 * Where the line starts: at the edge, 16 pt in (Settings rows), or 52 pt in, under the
 * text of a row of the conversations sheet and past its icon column (S-Rows).
 */
export type HairlineInset = "none" | "inset" | "row";

const INSET_CLASS = {
  none: "",
  inset: "ml-inset",
  row: "ml-52",
} as const satisfies Record<HairlineInset, string>;

export interface HairlineProps {
  readonly inset?: HairlineInset;
  /** True for a line that takes the free width of a row, as around the turn end text. */
  readonly grow?: boolean;
}

/** A horizontal hairline. VoiceOver skips it. */
export function Hairline(props: HairlineProps) {
  return (
    <View
      testID="hairline"
      accessibilityElementsHidden
      importantForAccessibility="no"
      className={cn(
        "h-hairline bg-border-subtle",
        INSET_CLASS[props.inset ?? "none"],
        props.grow === true && "flex-1",
      )}
    />
  );
}
