// What the All conversations sheet shows besides its sections (design-spec section 3.15,
// board R-AllSheet): the rows of the results pane while the search field has text, the
// quiet lines of its empty, loading and failed states, and the order of the machines.
// Pure, for the tests.
//
// The search has two sources, so that a row shows at once and a deep hit follows:
//
// - The typed text filters the loaded rows at once (`filterConversations`), by title,
//   machine name and cwd. efrd does not know the machine names, so only this filter
//   finds a row by its machine.
// - The search of efrd runs on the debounced text and also finds words inside the
//   conversations. Its rows come first, best first, once the debounced text is the
//   typed text. The local rows that efrd did not give follow, newest first.

import type { ConversationItem } from "./conversation-item";
import {
  filterConversations,
  mergeConversationLists,
  searchResultRows,
  sheetRow,
  type MachineName,
  type SheetRow,
} from "./sheet-sections";

/** How long the sheet waits after the last key before it asks efrd. */
export const SEARCH_DEBOUNCE_MS = 200;

/** The pane of the sheet: the sections, or the results of the search field. */
export type SheetPane = "sections" | "results";

/** The pane for the text of the search field. Spaces alone keep the sections. */
export function sheetPane(text: string): SheetPane {
  return text.trim().length === 0 ? "sections" : "results";
}

/** The sources of the results pane. */
export interface SearchPaneInput {
  /** The lists that the sections show (every machine, the empty query). */
  readonly loaded: readonly (readonly ConversationItem[])[];
  /** The search results of each machine for `searched`, the machine on screen first. */
  readonly found: readonly (readonly ConversationItem[])[];
  /** The text of the search field (deferred). */
  readonly text: string;
  /** The debounced text that `found` is for. */
  readonly searched: string;
  readonly machineName: MachineName;
  readonly now: number;
}

/**
 * The rows of the results pane: the rows of efrd first (only when they are for the
 * text that is typed now), then the loaded rows that match the text and that efrd did
 * not give. Each conversation shows once.
 */
export function searchPaneRows(input: SearchPaneInput): SheetRow[] {
  const current = input.searched.trim() === input.text.trim();
  const rows = current ? searchResultRows(input.found, input.machineName, input.now) : [];
  const shown = new Set(rows.map((row) => `${row.machine}:${row.id}`));
  const local = filterConversations(
    mergeConversationLists(input.loaded),
    input.text,
    input.machineName,
  );
  for (const item of local) {
    const key = `${item.machine}:${item.id}`;
    if (!shown.has(key)) {
      shown.add(key);
      rows.push(sheetRow(item, input.machineName, input.now));
    }
  }
  return rows;
}

/** The header of the results pane: `results · 3`. */
export function resultsTitle(count: number): string {
  return `results · ${String(count)}`;
}

/**
 * The quiet line of a pane, in mono. `loading` and `searching` show the braille cell
 * in text_muted before the words, because the app waits on the network.
 */
export type SheetNotice =
  | { readonly kind: "none" }
  | { readonly kind: "loading" | "searching" | "empty"; readonly text: string };

/** What the sections pane knows about its data. */
export interface SectionsNoticeInput {
  /** The number of paired machines. */
  readonly machines: number;
  /** True while no machine has data yet and one still fetches. */
  readonly pending: boolean;
  /** The number of rows of all sections. */
  readonly rows: number;
}

/** The line of the sections pane when it has no rows. */
export function sectionsNotice(input: SectionsNoticeInput): SheetNotice {
  if (input.rows > 0) {
    return { kind: "none" };
  }
  if (input.machines === 0) {
    return { kind: "empty", text: "no machine is paired" };
  }
  if (input.pending) {
    return { kind: "loading", text: "loading conversations" };
  }
  return { kind: "empty", text: "no conversations yet" };
}

/** What the results pane knows about the search. */
export interface ResultsNoticeInput {
  /** The number of rows of the results pane. */
  readonly rows: number;
  /** True while a machine runs the search. */
  readonly fetching: boolean;
  /** True when the debounced text is not the typed text yet. */
  readonly waiting: boolean;
}

/**
 * The line of the results pane: `searching` while efrd still looks (under the rows when
 * there are some), and `no conversations match` when it found nothing.
 */
export function resultsNotice(input: ResultsNoticeInput): SheetNotice {
  if (input.fetching || input.waiting) {
    return { kind: "searching", text: "searching" };
  }
  if (input.rows === 0) {
    return { kind: "empty", text: "no conversations match" };
  }
  return { kind: "none" };
}

/** The line under a pane when machines did not answer: `1 machine did not answer`. */
export function failedText(failed: number): string | undefined {
  if (failed <= 0) {
    return undefined;
  }
  return failed === 1 ? "1 machine did not answer" : `${String(failed)} machines did not answer`;
}

/** A paired machine as the sheet needs it. */
export interface SheetMachine {
  /** The `daemonId`. */
  readonly id: string;
  /** The short name, such as `desktop`. */
  readonly name: string;
}

/**
 * The machines in the order of the queries: the machine on screen first (its search
 * hits come first), then the others in the order of the registry.
 */
export function sheetMachines(
  machines: readonly SheetMachine[],
  first: string | undefined,
): SheetMachine[] {
  const front = machines.filter((machine) => machine.id === first);
  const rest = machines.filter((machine) => machine.id !== first);
  return [...front, ...rest];
}

/**
 * The name of a machine by its id. A row of a machine that is gone (forgotten while the
 * sheet is open) shows the start of its id.
 */
export function machineNameOf(machines: readonly SheetMachine[]): MachineName {
  return (id) => machines.find((machine) => machine.id === id)?.name ?? id.slice(0, 8);
}
