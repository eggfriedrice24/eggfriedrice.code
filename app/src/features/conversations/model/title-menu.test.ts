import { describe, expect, it } from "vitest";

import { october, item } from "./testing";
import { TITLE_MENU_ROWS, titleMenuRows } from "./title-menu";

const NOW = october(26, 14);
const MINUTE = 60_000;

// The sample of board R-TitleMenu, in a mixed order.
const DESKTOP = [
  item("desktop", "fan", NOW - 2 * MINUTE, { title: "Why is the fan loud", cwd: "/home/me" }),
  item("desktop", "expo", NOW - 60 * MINUTE, {
    title: "Expo SDK 58 upgrade",
    cwd: "/home/me/code/nimbus-v2",
  }),
  item("desktop", "migrate", NOW - 5 * MINUTE, {
    title: "Migrate session store to SQLite",
    cwd: "/home/me/code/efr",
    status: "running",
  }),
  item("desktop", "restic", october(25, 9), { title: "Set up restic backups", cwd: "/home/me" }),
  item("desktop", "zsh", october(24, 9), {
    title: "zsh startup is slow",
    cwd: "/home/me/.dotfiles",
  }),
  item("desktop", "docker", october(19, 9), {
    title: "Clean up old docker images",
    cwd: "/home/me",
  }),
  item("desktop", "handshake", october(12, 9), {
    title: "Handshake version check",
    cwd: "/home/me/code/efr",
  }),
  item("desktop", "steer", october(1), {
    title: "Fix steer race in turn loop",
    cwd: "/home/me/code/efr",
    status: "awaiting_approval",
  }),
  item("desktop", "old", october(1, 8), { title: "An old one" }),
];

describe("titleMenuRows", () => {
  it("shows the waiting ones first, then the newest, at most 8", () => {
    const rows = titleMenuRows({
      machine: "desktop",
      items: DESKTOP,
      current: { machine: "desktop", id: "fan" },
      now: NOW,
    });
    expect(rows).toHaveLength(TITLE_MENU_ROWS);
    expect(rows.map((row) => [row.title, row.subtitle, row.glyph, row.current])).toEqual([
      ["Fix steer race in turn loop", "Needs approval · ~/code/efr", "approval", false],
      ["Why is the fan loud", "~ · 2m", "none", true],
      ["Migrate session store to SQLite", "Running · ~/code/efr", "running", false],
      ["Expo SDK 58 upgrade", "~/code/nimbus-v2 · 1h", "none", false],
      ["Set up restic backups", "~ · Yesterday", "none", false],
      ["zsh startup is slow", "~/.dotfiles · Sat", "none", false],
      ["Clean up old docker images", "~ · Oct 19", "none", false],
      ["Handshake version check", "~/code/efr · Oct 12", "none", false],
    ]);
  });

  it("checks no row on the blank root", () => {
    const rows = titleMenuRows({
      machine: "desktop",
      items: DESKTOP,
      current: undefined,
      now: NOW,
    });
    expect(rows.some((row) => row.current)).toBe(false);
  });

  it("leaves out other machines and a second copy of a conversation", () => {
    const rows = titleMenuRows({
      machine: "desktop",
      items: [
        item("laptop", "x", NOW, { title: "On the laptop" }),
        item("desktop", "a", NOW - MINUTE, { title: "First copy" }),
        item("desktop", "a", NOW - 2 * MINUTE, { title: "Second copy" }),
      ],
      current: { machine: "laptop", id: "x" },
      now: NOW,
    });
    expect(rows.map((row) => row.title)).toEqual(["First copy"]);
    expect(rows[0]?.current).toBe(false);
  });

  it("names a conversation without a title or cwd", () => {
    const rows = titleMenuRows({
      machine: "m",
      items: [
        item("m", "a", NOW - 3 * MINUTE),
        item("m", "b", NOW - 4 * MINUTE, { status: "running" }),
      ],
      current: undefined,
      now: NOW,
    });
    expect(rows.map((row) => [row.title, row.subtitle])).toEqual([
      ["Untitled", "3m"],
      ["Untitled", "Running · 4m"],
    ]);
  });

  it("gives no rows for an empty list", () => {
    expect(titleMenuRows({ machine: "m", items: [], current: undefined, now: NOW })).toEqual([]);
  });
});
