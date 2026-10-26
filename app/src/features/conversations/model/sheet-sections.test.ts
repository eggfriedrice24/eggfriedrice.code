import { describe, expect, it } from "vitest";

import {
  filterConversations,
  mergeConversationLists,
  searchResultRows,
  sheetRow,
  sheetSections,
} from "./sheet-sections";
import { item, october } from "./testing";

const NOW = october(26, 14);
const MINUTE = 60_000;
const NAMES: Readonly<Record<string, string>> = { d: "desktop", l: "laptop" };
const machineName = (machine: string): string => NAMES[machine] ?? machine;

// The sample of board R-AllSheet.
const DESKTOP = [
  item("d", "steer", NOW - 20 * MINUTE, {
    title: "Fix steer race in turn loop",
    cwd: "/home/me/code/efr",
    status: "awaiting_approval",
  }),
  item("d", "fan", NOW - 2 * MINUTE, { title: "Why is the fan loud", cwd: "/home/me" }),
  item("d", "migrate", NOW - 3 * MINUTE, {
    title: "Migrate session store to SQLite",
    cwd: "/home/me/code/efr",
    status: "running",
  }),
  item("d", "expo", NOW - 60 * MINUTE, {
    title: "Expo SDK 58 upgrade",
    cwd: "/home/me/code/nimbus-v2",
  }),
  item("d", "restic", october(25, 20), { title: "Set up restic backups", cwd: "/home/me" }),
  item("d", "zsh", october(21, 9), {
    title: "zsh startup is slow",
    cwd: "/home/me/.dotfiles",
  }),
  item("d", "docker", october(19, 9), { title: "Clean up old docker images", cwd: "/home/me" }),
];
const LAPTOP = [
  item("l", "btrfs", NOW - 30 * MINUTE, {
    title: "Prune old btrfs snapshots",
    cwd: "/home/me",
    status: "awaiting_approval",
  }),
  item("l", "offline", NOW - 3 * 60 * MINUTE, {
    title: "Add offline queue for uploads",
    cwd: "/home/me/code/nimbus-v2",
  }),
  item("l", "handshake", october(25, 10), {
    title: "Handshake version check",
    cwd: "/home/me/code/efr",
  }),
  item("l", "starship", october(24, 9), {
    title: "Starship segment for efr status",
    cwd: "/home/me/.dotfiles",
  }),
];

describe("sheetSections", () => {
  it("puts the waiting ones on top, then today, this week and older", () => {
    const sections = sheetSections([...DESKTOP, ...LAPTOP], machineName, NOW);
    expect(
      sections.map((section) => [
        section.title,
        section.rows.map((row) => [row.title, row.status, row.detail, row.mark, row.bright]),
      ]),
    ).toEqual([
      [
        "waiting · 2",
        [
          [
            "Fix steer race in turn loop",
            "needs approval",
            "desktop · ~/code/efr",
            "approval",
            true,
          ],
          ["Prune old btrfs snapshots", "needs approval", "laptop · ~", "approval", true],
        ],
      ],
      [
        "today",
        [
          ["Why is the fan loud", undefined, "desktop · ~ · 2m", "none", false],
          ["Migrate session store to SQLite", "running", "desktop · ~/code/efr", "running", false],
          ["Expo SDK 58 upgrade", undefined, "desktop · ~/code/nimbus-v2 · 1h", "none", false],
          [
            "Add offline queue for uploads",
            undefined,
            "laptop · ~/code/nimbus-v2 · 3h",
            "none",
            false,
          ],
        ],
      ],
      [
        "this week",
        [
          ["Set up restic backups", undefined, "desktop · ~ · yesterday", "none", false],
          ["Handshake version check", undefined, "laptop · ~/code/efr · yesterday", "none", false],
          [
            "Starship segment for efr status",
            undefined,
            "laptop · ~/.dotfiles · Sat",
            "none",
            false,
          ],
          ["zsh startup is slow", undefined, "desktop · ~/.dotfiles · Wed", "none", false],
        ],
      ],
      ["older", [["Clean up old docker images", undefined, "desktop · ~ · Oct 19", "none", false]]],
    ]);
  });

  it("leaves out the empty sections", () => {
    const sections = sheetSections(
      [item("d", "a", NOW, { title: "Only today" })],
      machineName,
      NOW,
    );
    expect(sections.map((section) => section.kind)).toEqual(["today"]);
    expect(sheetSections([], machineName, NOW)).toEqual([]);
  });

  it("puts a conversation from the start of the week day in this week", () => {
    const start = october(20, 0, 0);
    const sections = sheetSections(
      [item("d", "edge", start), item("d", "before", start - 1)],
      machineName,
      NOW,
    );
    expect(sections.map((section) => [section.kind, section.rows.map((row) => row.id)])).toEqual([
      ["week", ["edge"]],
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
      machineName,
      NOW,
    );
    expect(rows.map((row) => `${row.machine}:${row.id}`)).toEqual(["d:1", "l:1", "d:2", "d:3"]);
    expect(rows.every((row) => !row.bright)).toBe(true);
  });
});

describe("sheetRow", () => {
  it("draws a row without cwd", () => {
    const row = sheetRow(item("x", "a", NOW - MINUTE, { title: "T" }), machineName, NOW);
    expect(row).toEqual({
      machine: "x",
      id: "a",
      title: "T",
      status: undefined,
      detail: "x · 1m",
      mark: "none",
      bright: false,
    });
  });
});
