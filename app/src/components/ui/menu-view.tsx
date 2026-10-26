// A native menu (UIMenu) on a view of ours: the title menu of the header, the row menu
// of Settings and the Steer/Queue pull-down of the composer. A tap opens it
// (`showsMenuAsPrimaryAction`); iOS cannot open a UIMenu from code. Menus are native, so
// they use the system font and cannot animate (design notes 3 and 12).
//
// With `openOnLongPress` a long press opens it instead (UIContextMenuInteraction), and a
// tap goes to the view inside: the Steer/Queue capsule sends on a tap, the Copy menus of
// the transcript let a tap toggle the block, and a row of the All conversations sheet
// opens on a tap.

import { MenuView as NativeMenuView } from "@react-native-menu/menu";
import type { ReactNode } from "react";

import { menuActions, type MenuEntry } from "./menu-actions";

export interface MenuViewProps {
  /** The title at the top of the menu, such as "Recent on desktop" or a machine name. */
  readonly title?: string;
  readonly entries: readonly MenuEntry[];
  /** Called with the id of the picked item. */
  readonly onAction: (id: string) => void;
  /** Called with true when the menu opens and with false when it closes. */
  readonly onOpenChange?: (open: boolean) => void;
  /**
   * True to open the menu on a long press, so that a tap reaches the children. The
   * default opens it on a tap.
   */
  readonly openOnLongPress?: boolean;
  /** The view that opens the menu on a tap. It holds its own label and role. */
  readonly children: ReactNode;
  readonly testID?: string;
}

/** A view that opens a native menu on a tap. */
export function MenuView(props: MenuViewProps) {
  const onOpenChange = props.onOpenChange;
  return (
    <NativeMenuView
      title={props.title ?? ""}
      actions={menuActions(props.entries)}
      shouldOpenOnLongPress={props.openOnLongPress === true}
      themeVariant="dark"
      onPressAction={({ nativeEvent }) => props.onAction(nativeEvent.event)}
      onOpenMenu={() => onOpenChange?.(true)}
      onCloseMenu={() => onOpenChange?.(false)}
      testID={props.testID ?? "menu-view"}
    >
      {props.children}
    </NativeMenuView>
  );
}
