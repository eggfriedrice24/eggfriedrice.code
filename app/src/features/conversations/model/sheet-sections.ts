// The rows and sections of the All conversations sheet (boards S-Rows and SH-Large).
// Pure.
//
// - Sections: "Needs you" and "Running" (every machine) lead and take their rows out of
//   the day sections. Then "Today", "Yesterday", "This week" (the 5 days before
//   yesterday) and "Older", each newest first. An empty section is left out.
// - The search field filters the loaded rows at once, by title, machine name and cwd
//   (`filterConversations`). The search of efrd (debounced) gives the results pane
//   (`searchResultRows`), best first per machine. A result shows the words of the search
//   in its title.
// - The row state (`rowState`, spec 0.0.12 section 6.1) gives the mark, the word and the
//   section: the three waits (needs approval, needs an answer, answer in the terminal)
//   have the yolk "!" and stand in Needs you; a running one has the loader and stands in
//   Running; a failed one has no mark, the word "✗ failed" in state_error, and stays in
//   its day.
// - A row of a machine that did not answer is offline (SH-Offline): it comes from the
//   last list that the phone keeps, so its title is quieter, the word "offline" follows
//   the machine, and it shows no status. A status from that list can be old, so the row
//   goes to its day section.

import { rowState, waitsForUser, type RowState } from "@/lib/protocol/status";
import { holdsEveryWord, matchRuns, searchWords, type MatchRun } from "@/lib/text/search-words";
import { newestFirst, type ConversationItem } from "./conversation-item";
import { folderName, listDay, listTime, rowTitle, spokenTime, statusWord } from "./list-format";

/** The sections of the sheet, in their order. */
export type SheetSectionKind = "needs" | "running" | "today" | "yesterday" | "week" | "older";

/** The icon of a row: the yolk "!" circle, the yolk braille loader, or the quiet bubble. */
export type SheetRowMark = "approval" | "running" | "none";

/** The colour role of the status word of a row. */
export type SheetStatusTone = "accent_primary" | "state_error";

/** One row of the sheet. */
export interface SheetRow {
  readonly machine: string;
  readonly id: string;
  readonly title: string;
  /** The title cut at the words of the search. One plain run without a search. */
  readonly titleRuns: readonly MatchRun[];
  /** The word before the machine, such as `needs approval` or `✗ failed`; none when idle. */
  readonly status: string | undefined;
  /** The colour of the status word: the yolk, or state_error for a failed turn. */
  readonly statusTone: SheetStatusTone;
  readonly mark: SheetRowMark;
  /** The name of the machine, such as `desktop`. */
  readonly machineName: string;
  /** True when the machine did not answer: the title is text_muted, "offline" follows. */
  readonly offline: boolean;
  /** The last part of the cwd in mono, `~` for home; none without a cwd. */
  readonly folder: string | undefined;
  /** When, in the short form: `now`, `12m`, `3h`, `18:40`, `Tue`, `Oct 12`. */
  readonly time: string;
  /** The row as one VoiceOver element: "Title. Needs approval. desktop, efr. Now." */
  readonly label: string;
  /** True in the Needs you section: the title is text_bright semibold. */
  readonly bright: boolean;
}

/** One section of the sheet. */
export interface SheetSection {
  readonly kind: SheetSectionKind;
  /** The header: `Needs you`, `Running`, `Today`, `Yesterday`, `This week` or `Older`. */
  readonly title: string;
  readonly rows: readonly SheetRow[];
}

/** The name of a machine by its `daemonId`. */
export type MachineName = (machine: string) => string;

/** What every row of a pane reads. */
export interface RowContext {
  readonly machineName: MachineName;
  /** The time of the list times, in ms since the epoch. */
  readonly now: number;
  /** The words of the search (`searchWords`), shown in the titles. None for the sections. */
  readonly words?: readonly string[];
  /** The machines that did not answer (`offlineMachines`). None: every machine answered. */
  readonly offline?: ReadonlySet<string>;
  /**
   * The machines on which the phone can answer a question of the model. None: no
   * machine, so a question reads as "answer in the terminal".
   */
  readonly answers?: ReadonlySet<string>;
}

/** The row state that the sheet trusts: idle for a row of a machine that did not answer. */
function liveState(item: ConversationItem, context: RowContext): RowState {
  if (context.offline?.has(item.machine) === true) {
    return "idle";
  }
  return rowState(item.status, item.attention, context.answers?.has(item.machine) === true);
}

const MARKS: Readonly<Record<RowState, SheetRowMark>> = {
  approval: "approval",
  answer: "approval",
  terminal: "approval",
  running: "running",
  failed: "none",
  idle: "none",
};

/** The status word of `state` as the row shows it: a failed turn has the ✗ before it. */
function shownStatus(state: RowState): string | undefined {
  const word = statusWord(state);
  return state === "failed" ? `✗ ${word ?? ""}` : word;
}

const SECTION_TITLES: Readonly<Record<SheetSectionKind, string>> = {
  needs: "Needs you",
  running: "Running",
  today: "Today",
  yesterday: "Yesterday",
  week: "This week",
  older: "Older",
};

const SECTION_ORDER: readonly SheetSectionKind[] = [
  "needs",
  "running",
  "today",
  "yesterday",
  "week",
  "older",
];

/** `text` as a sentence of a VoiceOver label: it ends with a stop. */
function sentence(text: string): string {
  return /[.?!]$/.test(text) ? text : `${text}.`;
}

function capitalized(text: string | undefined): string | undefined {
  return text === undefined ? undefined : text.charAt(0).toUpperCase() + text.slice(1);
}

/** The sheet row of `item`. */
export function sheetRow(item: ConversationItem, context: RowContext, bright = false): SheetRow {
  const title = rowTitle(item.title);
  const state = liveState(item, context);
  const offline = context.offline?.has(item.machine) === true;
  const machineName = context.machineName(item.machine);
  const folder = item.cwd === undefined ? undefined : folderName(item.cwd);
  const place = [machineName, offline ? "offline" : undefined, folder]
    .filter((part) => part !== undefined)
    .join(", ");
  const sentences = [
    title,
    capitalized(statusWord(state)),
    place,
    spokenTime(item.updatedAt, context.now),
  ];
  return {
    machine: item.machine,
    id: item.id,
    title,
    titleRuns: matchRuns(title, context.words ?? []),
    status: shownStatus(state),
    statusTone: state === "failed" ? "state_error" : "accent_primary",
    mark: MARKS[state],
    machineName,
    offline,
    folder,
    time: listTime(item.updatedAt, context.now),
    label: sentences
      .filter((part): part is string => part !== undefined && part !== "")
      .map(sentence)
      .join(" "),
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
  const words = searchWords(text);
  if (words.length === 0) {
    return [...items];
  }
  return items.filter((item) =>
    holdsEveryWord(words, [item.title ?? "", machineName(item.machine), item.cwd ?? ""]),
  );
}

/** The section of `item`: a wait or a running turn first, else the day of its last event. */
function sectionOf(item: ConversationItem, context: RowContext): SheetSectionKind {
  const state = liveState(item, context);
  if (waitsForUser(state)) {
    return "needs";
  }
  return state === "running" ? "running" : listDay(item.updatedAt, context.now);
}

/** The sections of the sheet over `items` (of every machine). */
export function sheetSections(
  items: readonly ConversationItem[],
  context: RowContext,
): SheetSection[] {
  const groups = new Map<SheetSectionKind, ConversationItem[]>();
  for (const item of mergeConversationLists([items])) {
    const kind = sectionOf(item, context);
    const group = groups.get(kind);
    if (group === undefined) {
      groups.set(kind, [item]);
    } else {
      group.push(item);
    }
  }
  const sections: SheetSection[] = [];
  for (const kind of SECTION_ORDER) {
    const group = groups.get(kind);
    if (group !== undefined) {
      const bright = kind === "needs";
      sections.push({
        kind,
        title: SECTION_TITLES[kind],
        rows: group.map((item) => sheetRow(item, context, bright)),
      });
    }
  }
  return sections;
}

/**
 * The rows of the results pane: the search results of each machine, best first. The
 * lists are taken in turn, one row of each, so that the best hits of every machine
 * come first. The first list is the machine on screen. A conversation shows once.
 */
export function searchResultRows(
  lists: readonly (readonly ConversationItem[])[],
  context: RowContext,
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
        rows.push(sheetRow(item, context));
      }
    }
  }
  return rows;
}
