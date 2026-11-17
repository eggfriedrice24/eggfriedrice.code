import { describe, expect, it } from "vitest";

import {
  filterConversations,
  mergeConversationLists,
  searchResultRows,
  sheetRow,
  sheetSections,
  type RowContext,
  type SheetRow,
} from "./sheet-sections";
import { item, october } from "./testing";

const NOW = october(26, 14);
const MINUTE = 60_000;
const NAMES: Readonly<Record<string, string>> = { d: "desktop", l: "laptop" };
const machineName = (machine: string): string => NAMES[machine] ?? machine;
const CONTEXT: RowContext = { machineName, now: NOW };

// The sample of board SH-Large.
const DESKTOP = [
  item("d", "steer", NOW - 20 * MINUTE, {
    title: "Fix steer race in turn loop",
    cwd: "/home/me/code/efr",
    status: "awaiting_approval",
  }),
  item("d", "fan", NOW - 12 * MINUTE, { title: "Why is the fan loud", cwd: "/home/me" }),
  item("d", "migrate", NOW - 4 * MINUTE, {
    title: "Migrate session store to SQLite",
    cwd: "/home/me/code/efr",
    status: "running",
  }),
  item("d", "expo", NOW - 60 * MINUTE, {
    title: "Expo SDK 58 upgrade",
    cwd: "/home/me/code/nimbus-v2",
  }),
  item("d", "restic", october(25, 18, 40), { title: "Set up restic backups", cwd: "/home/me" }),
  item("d", "zsh", october(21, 9), {
    title: "zsh startup is slow",
    cwd: "/home/me/.dotfiles",
  }),
  item("d", "docker", october(19, 9), { title: "Clean up old docker images", cwd: "/home/me" }),
];
const LAPTOP = [
  item("l", "btrfs", NOW - 3 * MINUTE, {
    title: "Prune old btrfs snapshots",
    cwd: "/home/me",
    status: "awaiting_approval",
  }),
  item("l", "offline", NOW - 3 * 60 * MINUTE, {
    title: "Add offline queue for uploads",
    cwd: "/home/me/code/nimbus-v2",
  }),
  item("l", "handshake", october(25, 11, 2), {
    title: "Handshake version check",
    cwd: "/home/me/code/efr",
  }),
  item("l", "starship", october(24, 9), {
    title: "Starship segment for efr status",
    cwd: "/home/me/.dotfiles",
  }),
];

/** A row as [title, status, machine, folder, time, mark, bright]. */
function shown(row: SheetRow): unknown[] {
  return [row.title, row.status, row.machineName, row.folder, row.time, row.mark, row.bright];
}

describe("sheetSections (S-Rows, Sections)", () => {
  it("leads with Needs you and Running, then the days, each newest first", () => {
    const sections = sheetSections([...DESKTOP, ...LAPTOP], CONTEXT);
    expect(sections.map((section) => [section.title, section.rows.map(shown)])).toEqual([
      [
        "Needs you",
        [
          ["Prune old btrfs snapshots", "needs approval", "laptop", "~", "3m", "approval", true],
          [
            "Fix steer race in turn loop",
            "needs approval",
            "desktop",
            "efr",
            "20m",
            "approval",
            true,
          ],
        ],
      ],
      [
        "Running",
        [["Migrate session store to SQLite", "running", "desktop", "efr", "4m", "running", false]],
      ],
      [
        "Today",
        [
          ["Why is the fan loud", undefined, "desktop", "~", "12m", "none", false],
          ["Expo SDK 58 upgrade", undefined, "desktop", "nimbus-v2", "1h", "none", false],
          ["Add offline queue for uploads", undefined, "laptop", "nimbus-v2", "3h", "none", false],
        ],
      ],
      [
        "Yesterday",
        [
          ["Set up restic backups", undefined, "desktop", "~", "18:40", "none", false],
          ["Handshake version check", undefined, "laptop", "efr", "11:02", "none", false],
        ],
      ],
      [
        "This week",
        [
          [
            "Starship segment for efr status",
            undefined,
            "laptop",
            ".dotfiles",
            "Sat",
            "none",
            false,
          ],
          ["zsh startup is slow", undefined, "desktop", ".dotfiles", "Wed", "none", false],
        ],
      ],
      [
        "Older",
        [["Clean up old docker images", undefined, "desktop", "~", "Oct 19", "none", false]],
      ],
    ]);
  });

  it("keeps a waiting or running conversation of another day in its status section", () => {
    const sections = sheetSections(
      [
        item("d", "old-wait", october(2), { status: "awaiting_approval" }),
        item("d", "old-run", october(3), { status: "running" }),
      ],
      CONTEXT,
    );
    expect(sections.map((section) => [section.kind, section.rows.map((row) => row.time)])).toEqual([
      ["needs", ["Oct 2"]],
      ["running", ["Oct 3"]],
    ]);
  });

  it("leaves out the empty sections", () => {
    const sections = sheetSections([item("d", "a", NOW, { title: "Only today" })], CONTEXT);
    expect(sections.map((section) => section.kind)).toEqual(["today"]);
    expect(sheetSections([], CONTEXT)).toEqual([]);
  });

  it("splits the days at local midnight", () => {
    const today = october(26, 0, 0);
    const yesterday = october(25, 0, 0);
    const week = october(20, 0, 0);
    const sections = sheetSections(
      [
        item("d", "today", today),
        item("d", "yesterday-end", today - 1),
        item("d", "yesterday", yesterday),
        item("d", "week-end", yesterday - 1),
        item("d", "week", week),
        item("d", "before", week - 1),
      ],
      CONTEXT,
    );
    expect(sections.map((section) => [section.kind, section.rows.map((row) => row.id)])).toEqual([
      ["today", ["today"]],
      ["yesterday", ["yesterday-end", "yesterday"]],
      ["week", ["week-end", "week"]],
      ["older", ["before"]],
    ]);
  });
});

describe("mergeConversationLists", () => {
  it("keeps one copy of each conversation, newest first", () => {
    const merged = mergeConversationLists([
      [item("d", "a", 1), item("d", "b", 3)],
      [item("l", "a", 2), item("d", "a", 9)],
    ]);
    expect(merged.map((row) => `${row.machine}:${row.id}:${row.updatedAt}`)).toEqual([
      "d:b:3",
      "l:a:2",
      "d:a:1",
    ]);
  });
});

describe("filterConversations", () => {
  const rows = [...DESKTOP, ...LAPTOP];

  it("finds every word in the title, the machine name or the cwd, ignoring case", () => {
    expect(filterConversations(rows, "STEER", machineName).map((row) => row.id)).toEqual(["steer"]);
    expect(filterConversations(rows, "laptop nimbus", machineName).map((row) => row.id)).toEqual([
      "offline",
    ]);
    expect(filterConversations(rows, ".dotfiles", machineName).map((row) => row.id)).toEqual([
      "zsh",
      "starship",
    ]);
  });

  it("keeps every row for an empty text and none for a word that matches nothing", () => {
    expect(filterConversations(rows, "  ", machineName)).toHaveLength(rows.length);
    expect(filterConversations(rows, "kernel", machineName)).toEqual([]);
  });
});

describe("searchResultRows", () => {
  it("takes the best hit of each machine in turn, and each conversation once", () => {
    const rows = searchResultRows(
      [
        [item("d", "1", 0, { title: "d best" }), item("d", "2", 0), item("d", "3", 0)],
        [item("l", "1", 0, { title: "l best" }), item("d", "2", 0)],
      ],
      CONTEXT,
    );
    expect(rows.map((row) => `${row.machine}:${row.id}`)).toEqual(["d:1", "l:1", "d:2", "d:3"]);
    expect(rows.every((row) => !row.bright)).toBe(true);
  });
});

describe("sheetRow", () => {
  it("draws a row without cwd, and reads it to VoiceOver as one element", () => {
    const row = sheetRow(item("x", "a", NOW - MINUTE, { title: "T" }), CONTEXT);
    expect(row).toEqual({
      machine: "x",
      id: "a",
      title: "T",
      titleRuns: [{ text: "T", start: 0, match: false }],
      status: undefined,
      mark: "none",
      machineName: "x",
      folder: undefined,
      time: "1m",
      label: "T. x. 1 minute ago.",
      bright: false,
    });
  });

  it("gives the label of the board", () => {
    const [steer] = DESKTOP;
    if (steer === undefined) {
      throw new Error("no sample");
    }
    const row = sheetRow({ ...steer, updatedAt: NOW }, CONTEXT, true);
    expect(row.label).toBe("Fix steer race in turn loop. Needs approval. desktop, efr. Now.");
    const question = sheetRow(item("d", "q", NOW, { title: "Why is the fan loud?" }), CONTEXT);
    expect(question.label).toBe("Why is the fan loud? desktop. Now.");
  });

  it("marks the words of the search in the title", () => {
    const row = sheetRow(item("l", "s", NOW, { title: "Steer and queue frames" }), {
      ...CONTEXT,
      words: ["steer"],
    });
    expect(row.titleRuns).toEqual([
      { text: "Steer", start: 0, match: true },
      { text: " and queue frames", start: 5, match: false },
    ]);
  });
});
