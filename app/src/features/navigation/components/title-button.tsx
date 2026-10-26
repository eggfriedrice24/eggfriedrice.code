// The title of the bar as a button that opens the title menu (design section 2): the
// title, a chevron and, while another conversation waits for an answer, the 7 pt yolk
// dot at the top right of the chevron. Open, the chevron turns up. It has no fill of its
// own: a fill inside a glass bar looks pasted on, and t3code has none. The blank root
// draws its machine name in mono 15 semibold; a conversation draws its title at the size
// of a native bar title, SF Pro 17 semibold. Both are one line.

import { useState } from "react";
import { Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { Dot } from "@/components/ui/dot";
import type { MenuEntry } from "@/components/ui/menu-actions";
import { MenuView } from "@/components/ui/menu-view";
import { SymbolIcon } from "@/components/ui/symbol";

export interface TitleButtonProps {
  readonly title: string;
  /** True for the machine name of the blank root. */
  readonly mono: boolean;
  /** True while another conversation waits for an answer. */
  readonly dot: boolean;
  /** The VoiceOver label, such as "desktop, recent conversations". */
  readonly label: string;
  /** The title of the menu, such as "Recent on desktop". */
  readonly menuTitle: string;
  readonly entries: readonly MenuEntry[];
  readonly onAction: (id: string) => void;
}

/** The title of the bar and its menu. */
export function TitleButton(props: TitleButtonProps) {
  const [open, setOpen] = useState(false);
  return (
    <MenuView
      title={props.menuTitle}
      entries={props.entries}
      onAction={props.onAction}
      onOpenChange={setOpen}
      testID="title-menu"
    >
      <View
        accessible
        accessibilityRole="button"
        accessibilityLabel={props.label}
        accessibilityState={{ expanded: open }}
        testID="title-button"
        className="h-hit flex-row items-center gap-5 px-6"
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
          <SymbolIcon
            name={open ? "chevron.up" : "chevron.down"}
            size={11}
            tone="text_muted"
            weight="heavy"
          />
          {props.dot ? (
            <View testID="title-dot" className="absolute -right-6 -top-5">
              <Dot size="small" />
            </View>
          ) : null}
        </View>
      </View>
    </MenuView>
  );
}
