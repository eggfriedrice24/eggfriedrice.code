// The rows of the title menu (design-spec section 3.14, board R-TitleMenu): the 8 most
// recent conversations of the current machine, the waiting ones first, then the newest.
// Pure. The menu itself is a system UIMenu, so each row is plain text and one glyph.

import type { ConversationStatus } from "@/lib/protocol/generated";
import type { ConversationAddress } from "@/lib/query/keys";
import { newestFirst, sameConversation, type ConversationItem } from "./conversation-item";
import { joinParts, listTime, rowTitle, shortPath, statusWord } from "./list-format";

/** The most rows of the title menu. */
export const TITLE_MENU_ROWS = 8;

/**
 * The glyph of a row: `approval` is the yolk exclamationmark.circle.fill, `running` a
 * still braille frame in yolk (a menu cannot animate), `none` no image.
 */
export type TitleMenuGlyph = "approval" | "running" | "none";

/** One row of the title menu. */
export interface TitleMenuRow {
  readonly machine: string;
  readonly id: string;
  readonly title: string;
  /** `Needs approval · ~/code/efr`, `Running · ~/code/efr` or `~/code/efr · 2m`. */
  readonly subtitle: string;
  readonly glyph: TitleMenuGlyph;
  /** True for the conversation on screen: the row has state `on` (a check). */
  readonly current: boolean;
}

/** What the title menu shows. */
export interface TitleMenuInput {
  /** The machine of the screen; rows of other machines are left out. */
  readonly machine: string;
  /** The recent conversations of that machine, in any order. */
  readonly items: readonly ConversationItem[];
  /** The conversation on screen; undefined on the blank root. */
  readonly current: ConversationAddress | undefined;
  /** The time now, in ms since the epoch. */
  readonly now: number;
}

function waitingFirst(a: ConversationItem, b: ConversationItem): number {
  const aWaits = a.status === "awaiting_approval";
  const bWaits = b.status === "awaiting_approval";
  if (aWaits !== bWaits) {
    return aWaits ? -1 : 1;
  }
  return newestFirst(a, b);
}

const GLYPHS: Readonly<Record<ConversationStatus, TitleMenuGlyph>> = {
  awaiting_approval: "approval",
  running: "running",
  idle: "none",
};

function subtitleOf(item: ConversationItem, now: number): string {
  const cwd = item.cwd === undefined ? undefined : shortPath(item.cwd);
  const word = statusWord(item.status);
  if (word !== undefined) {
    return joinParts([word, cwd ?? listTime(item.updatedAt, now)]);
  }
  return joinParts([cwd, listTime(item.updatedAt, now)]);
}

/** The rows of the title menu, at most TITLE_MENU_ROWS. */
export function titleMenuRows(input: TitleMenuInput): TitleMenuRow[] {
  const seen = new Set<string>();
  const items: ConversationItem[] = [];
  for (const item of input.items) {
    if (item.machine === input.machine && !seen.has(item.id)) {
      seen.add(item.id);
      items.push(item);
    }
  }
  items.sort(waitingFirst);
  return items.slice(0, TITLE_MENU_ROWS).map((item) => ({
    machine: item.machine,
    id: item.id,
    title: rowTitle(item.title),
    subtitle: subtitleOf(item, input.now),
    glyph: GLYPHS[item.status],
    current: input.current !== undefined && sameConversation(item, input.current),
  }));
}
