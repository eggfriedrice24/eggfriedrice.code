// The items of the title menu (design-spec section 3.14, board R-TitleMenu) and the
// VoiceOver label of the title. The rows come from `titleMenuRows` of the conversations
// feature; this file makes them the items of the native menu. Pure.
//
// - An approval row has the yolk exclamationmark.circle.fill, a running row a still yolk
//   glyph (a menu cannot animate), an idle row no image. Both glyphs keep their colour
//   (design item 8.11).
// - The conversation on screen has the check (state `on`).
// - "All conversations…" follows in its own section and opens the sheet.

import type { MenuEntry, MenuItem } from "@/components/ui/menu-actions";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type { TitleMenuGlyph, TitleMenuRow } from "@/features/conversations/model/title-menu";

/** The id of the "All conversations…" item. */
export const ALL_CONVERSATIONS = "all";

const OPEN_PREFIX = "open:";

/** The SF Symbol of each glyph of a row. */
export const TITLE_MENU_SYMBOLS: Readonly<Record<Exclude<TitleMenuGlyph, "none">, string>> = {
  approval: "exclamationmark.circle.fill",
  // NOTE: A UIMenu takes only images, so the still braille frame of the design is an SF
  // Symbol of dots here (device check of plan section 6).
  running: "circle.dotted",
};

function rowItem(row: TitleMenuRow): MenuItem {
  const item: {
    id: string;
    title: string;
    subtitle?: string;
    image?: string;
    imageTone?: "accent_primary";
    state?: "on";
  } = { id: `${OPEN_PREFIX}${row.machine}:${row.id}`, title: row.title };
  if (row.subtitle !== "") {
    item.subtitle = row.subtitle;
  }
  if (row.glyph !== "none") {
    item.image = TITLE_MENU_SYMBOLS[row.glyph];
    item.imageTone = "accent_primary";
  }
  if (row.current) {
    item.state = "on";
  }
  return item;
}

/** The items of the title menu for `rows`. */
export function titleMenuEntries(rows: readonly TitleMenuRow[]): MenuEntry[] {
  return [
    ...rows.map(rowItem),
    {
      section: "all-conversations",
      items: [{ id: ALL_CONVERSATIONS, title: "All conversations…", image: "magnifyingglass" }],
    },
  ];
}

/** What a pick in the title menu does. */
export type TitleMenuAction =
  | { readonly kind: "open"; readonly conversation: ConversationRef }
  | { readonly kind: "all" };

/** The action of the item with id `id`, or undefined for an id that the menu did not make. */
export function titleMenuAction(id: string): TitleMenuAction | undefined {
  if (id === ALL_CONVERSATIONS) {
    return { kind: "all" };
  }
  if (!id.startsWith(OPEN_PREFIX)) {
    return undefined;
  }
  const rest = id.slice(OPEN_PREFIX.length);
  const colon = rest.indexOf(":");
  if (colon <= 0 || colon === rest.length - 1) {
    return undefined;
  }
  return {
    kind: "open",
    conversation: { machine: rest.slice(0, colon), id: rest.slice(colon + 1) },
  };
}

/** The title of the menu: "Recent on desktop". */
export function titleMenuTitle(machineName: string): string {
  return `Recent on ${machineName}`;
}

/**
 * The VoiceOver label of the title: "desktop, recent conversations", and ", 1 approval
 * waiting" while the dot shows.
 */
export function titleLabel(title: string, waiting: number): string {
  const base = `${title}, recent conversations`;
  if (waiting <= 0) {
    return base;
  }
  return waiting === 1 ? `${base}, 1 approval waiting` : `${base}, ${waiting} approvals waiting`;
}
