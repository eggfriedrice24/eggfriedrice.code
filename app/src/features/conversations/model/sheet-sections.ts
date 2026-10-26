// The rows and sections of the All conversations sheet (design-spec section 3.15, board
// R-AllSheet). Pure.
//
// - Sections: `waiting · N` (every machine, on top), `today`, `this week` (the 6 days
//   before today) and `older`, each newest first. A waiting conversation is only in the
//   waiting section. An empty section is left out.
// - The search field filters the loaded rows at once, by title, machine name and cwd
//   (`filterConversations`). The search of efrd (debounced) gives the results pane
//   (`searchResultRows`), best first per machine.

import type { ConversationStatus } from "@/lib/protocol/generated";
import { newestFirst, type ConversationItem } from "./conversation-item";
import {
  joinParts,
  listTime,
  rowTitle,
  shortPath,
  startOfLocalDay,
  startOfLocalDayBefore,
  statusWord,
} from "./list-format";

/** The sections of the sheet, in their order. */
export type SheetSectionKind = "waiting" | "today" | "week" | "older";

/** The trailing mark of a row: the 8 pt yolk dot, the braille cell, or none. */
export type SheetRowMark = "approval" | "running" | "none";

/** One row of the sheet. */
export interface SheetRow {
  readonly machine: string;
  readonly id: string;
  readonly title: string;
  /** `needs approval` or `running`, drawn in yolk before the detail; none when idle. */
  readonly status: string | undefined;
  /** `desktop · ~/code/efr` after a status word, else `desktop · ~ · 2m`. */
  readonly detail: string;
  readonly mark: SheetRowMark;
  /** True in the waiting section: the title is text_bright. */
  readonly bright: boolean;
}

/** One section of the sheet. */
export interface SheetSection {
  readonly kind: SheetSectionKind;
  /** The mono header: `waiting · 2`, `today`, `this week` or `older`. */
  readonly title: string;
  readonly rows: readonly SheetRow[];
}

/** The name of a machine by its `daemonId`. */
export type MachineName = (machine: string) => string;

const MARKS: Readonly<Record<ConversationStatus, SheetRowMark>> = {
  awaiting_approval: "approval",
  running: "running",
  idle: "none",
};

/** The sheet row of `item`. */
export function sheetRow(
  item: ConversationItem,
  machineName: MachineName,
  now: number,
  bright = false,
): SheetRow {
  const status = statusWord(item.status, "sheet");
  const cwd = item.cwd === undefined ? undefined : shortPath(item.cwd);
  const time = status === undefined ? listTime(item.updatedAt, now, "sheet") : undefined;
  return {
    machine: item.machine,
    id: item.id,
    title: rowTitle(item.title),
    status,
    detail: joinParts([machineName(item.machine), cwd, time]),
    mark: MARKS[item.status],
    bright,
  };
}

/**
 * The lists of several machines as one: each conversation once (the first copy wins),
 * newest first.
 */
export function mergeConversationLists(
  lists: readonly (readonly ConversationItem[])[],
): ConversationItem[] {
  const seen = new Set<string>();
  const merged: ConversationItem[] = [];
  for (const list of lists) {
    for (const item of list) {
      const key = `${item.machine}:${item.id}`;
      if (!seen.has(key)) {
        seen.add(key);
        merged.push(item);
      }
    }
  }
  merged.sort(newestFirst);
  return merged;
}

/**
 * The items whose title, machine name or cwd holds every word of `text`, ignoring case.
 * An empty text keeps all items.
 */
export function filterConversations(
  items: readonly ConversationItem[],
  text: string,
  machineName: MachineName,
): ConversationItem[] {
  const words = text
    .toLowerCase()
    .split(/\s+/)
    .filter((word) => word.length > 0);
  if (words.length === 0) {
    return [...items];
  }
  return items.filter((item) => {
    const haystack = [item.title ?? "", machineName(item.machine), item.cwd ?? ""]
      .join("\n")
      .toLowerCase();
    return words.every((word) => haystack.includes(word));
  });
}

/** The sections of the sheet over `items` (of every machine), at `now`. */
export function sheetSections(
  items: readonly ConversationItem[],
  machineName: MachineName,
  now: number,
): SheetSection[] {
  const today = startOfLocalDay(now);
  const weekStart = startOfLocalDayBefore(now, 6);
  const groups: Record<SheetSectionKind, ConversationItem[]> = {
    waiting: [],
    today: [],
    week: [],
    older: [],
  };
  for (const item of mergeConversationLists([items])) {
    if (item.status === "awaiting_approval") {
      groups.waiting.push(item);
    } else if (item.updatedAt >= today) {
      groups.today.push(item);
    } else if (item.updatedAt >= weekStart) {
      groups.week.push(item);
    } else {
      groups.older.push(item);
    }
  }
  const sections: SheetSection[] = [];
  const add = (kind: SheetSectionKind, title: string): void => {
    const group = groups[kind];
    if (group.length > 0) {
      const bright = kind === "waiting";
      sections.push({
        kind,
        title,
        rows: group.map((item) => sheetRow(item, machineName, now, bright)),
      });
    }
  };
  add("waiting", `waiting · ${groups.waiting.length}`);
  add("today", "today");
  add("week", "this week");
  add("older", "older");
  return sections;
}

/**
 * The rows of the results pane: the search results of each machine, best first. The
 * lists are taken in turn, one row of each, so that the best hits of every machine
 * come first. The first list is the machine on screen. A conversation shows once.
 */
export function searchResultRows(
  lists: readonly (readonly ConversationItem[])[],
  machineName: MachineName,
  now: number,
): SheetRow[] {
  const seen = new Set<string>();
  const rows: SheetRow[] = [];
  const longest = lists.reduce((most, list) => Math.max(most, list.length), 0);
  for (let index = 0; index < longest; index += 1) {
    for (const list of lists) {
      const item = list[index];
      if (item === undefined) {
        continue;
      }
      const key = `${item.machine}:${item.id}`;
      if (!seen.has(key)) {
        seen.add(key);
        rows.push(sheetRow(item, machineName, now));
      }
    }
  }
  return rows;
}
