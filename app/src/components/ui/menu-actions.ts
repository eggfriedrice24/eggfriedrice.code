// The items of a native menu (UIMenu through @react-native-menu/menu) as our own small
// type, and their conversion to the actions of the package.
//
// - An item never gets an empty image. @react-native-menu/menu 2.0.0 logs a CoreUI fault
//   (about 50 ms) for an empty icon, so an item without a symbol sends no `image` key.
// - A glyph that must keep its colour (the yolk glyphs of the title menu, design item
//   8.11) sets `imageTone`: the package then draws the image in that colour instead of
//   the label colour.
// - A section is an inline group (`displayInline`): iOS draws it after a gap, such as
//   "Pair a new machine" under the machines.

import type { MenuAction } from "@react-native-menu/menu";

import { roles, type Role } from "@/theme/roles";

/** One action of a menu. */
export interface MenuItem {
  /** The id that `onAction` gets. */
  readonly id: string;
  readonly title: string;
  /** The second line, such as "Connected" or "Needs approval · ~/code/efr". */
  readonly subtitle?: string;
  /** An SF Symbol name. Leave it out for no image; never pass "". */
  readonly image?: string;
  /** A colour role that the image keeps. Without it the image takes the label colour. */
  readonly imageTone?: Role;
  /** "on" draws the check of the current item. */
  readonly state?: "on" | "off" | "mixed";
  /** A destructive action, such as "Forget this machine". iOS draws it in system red. */
  readonly destructive?: boolean;
  readonly disabled?: boolean;
}

/** An inline section of a menu: its items follow a gap. */
export interface MenuSection {
  readonly section: string;
  readonly items: readonly MenuItem[];
}

/** One entry of a menu: an action, or an inline section of actions. */
export type MenuEntry = MenuItem | MenuSection;

function isSection(entry: MenuEntry): entry is MenuSection {
  return "section" in entry;
}

function toAction(item: MenuItem): MenuAction {
  const action: MenuAction = { id: item.id, title: item.title };
  if (item.subtitle !== undefined && item.subtitle !== "") {
    action.subtitle = item.subtitle;
  }
  if (item.image !== undefined && item.image !== "") {
    action.image = item.image;
    if (item.imageTone !== undefined) {
      action.imageColor = roles[item.imageTone];
    }
  }
  if (item.state !== undefined) {
    action.state = item.state;
  }
  if (item.destructive === true || item.disabled === true) {
    const attributes: NonNullable<MenuAction["attributes"]> = {};
    if (item.destructive === true) {
      attributes.destructive = true;
    }
    if (item.disabled === true) {
      attributes.disabled = true;
    }
    action.attributes = attributes;
  }
  return action;
}

/** The actions of the package for `entries`, in order. */
export function menuActions(entries: readonly MenuEntry[]): MenuAction[] {
  return entries.map((entry) =>
    isSection(entry)
      ? {
          id: entry.section,
          title: "",
          displayInline: true,
          subactions: entry.items.map(toAction),
        }
      : toAction(entry),
  );
}
