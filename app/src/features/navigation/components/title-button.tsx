// The title of the bar as a button that opens the conversations sheet (design section 2,
// changed 2026-10-26): the title, a chevron and, while another conversation waits for an
// answer, the 7 pt yolk dot at the top right of the chevron. The sheet is the one place
// to switch conversations. The button has no fill of its own: a fill inside a glass bar
// looks pasted on, and t3code has none. The blank root draws its machine name in mono 15
// semibold; a conversation draws its title at the size of a native bar title, SF Pro 17
// semibold. Both are one line.

import { Pressable, Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { Dot } from "@/components/ui/dot";
import { SymbolIcon } from "@/components/ui/symbol";

export interface TitleButtonProps {
  readonly title: string;
  /** True for the machine name of the blank root. */
  readonly mono: boolean;
  /** True while another conversation waits for an answer. */
  readonly dot: boolean;
  /** The VoiceOver label, such as "desktop, 1 waiting". */
  readonly label: string;
  /** Opens the conversations sheet. */
  readonly onPress: () => void;
}

/** The title of the bar. */
export function TitleButton(props: TitleButtonProps) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.label}
      accessibilityHint="Shows all conversations."
      onPress={props.onPress}
      testID="title-button"
      className="h-hit flex-row items-center gap-5 px-6 active:opacity-60"
    >
      <Text
        numberOfLines={1}
        className={cn(
          "shrink text-text-bright",
          props.mono ? "max-w-200 text-mono-title" : "max-w-200 text-nav-title",
        )}
      >
        {props.title}
      </Text>
      <View className="relative">
        <SymbolIcon name="chevron.down" size={11} tone="text_muted" weight="heavy" />
        {props.dot ? (
          <View testID="title-dot" className="absolute -right-6 -top-5">
            <Dot size="small" />
          </View>
        ) : null}
      </View>
    </Pressable>
  );
}
