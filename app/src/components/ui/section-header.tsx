// The section header of the All conversations sheet (S-Rows "Sections", SH-Large): SF Pro
// 13/18 semibold in text_subtle, sentence case, such as "Needs you" or "Today". It is
// plain: no hairline and no fill of its own. It sticks while its rows scroll, and its
// background is the surface_1 of the sheet, so it shows no band at rest and the rows do
// not show through it while it sticks.

import { Text, View } from "react-native";

export interface SectionHeaderProps {
  readonly title: string;
}

/** A sticky, plain section header. */
export function SectionHeader(props: SectionHeaderProps) {
  return (
    <View className="bg-surface-1">
      <Text
        accessibilityRole="header"
        className="px-gutter pt-16 pb-6 text-section-header text-text-subtle"
      >
        {props.title}
      </Text>
    </View>
  );
}
