// The sticky section header of the All conversations sheet (design 3.15): mono 11/15 in
// text_subtle with a hairline under it, such as "today" or "this week". The waiting
// section is yolk and shows its count: "waiting · 2". It has the sheet background, so
// the rows do not show through it while it sticks.

import { Text, View } from "react-native";

import { cn } from "./cn";
import { Hairline } from "./hairline";

export interface SectionHeaderProps {
  readonly title: string;
  /** The number of rows, shown after the title as "· n". */
  readonly count?: number;
  /** True for the waiting section, in yolk. */
  readonly waiting?: boolean;
}

/** The text of a section header: "waiting · 2", or the title alone. */
export function sectionHeaderText(title: string, count: number | undefined): string {
  return count === undefined ? title : `${title} · ${String(count)}`;
}

/** A sticky section header. */
export function SectionHeader(props: SectionHeaderProps) {
  return (
    <View className="bg-surface-1">
      <Text
        accessibilityRole="header"
        className={cn(
          "px-gutter pt-14 pb-6 text-mono-xs-tight",
          props.waiting === true ? "text-accent-primary" : "text-text-subtle",
        )}
      >
        {sectionHeaderText(props.title, props.count)}
      </Text>
      <Hairline />
    </View>
  );
}
