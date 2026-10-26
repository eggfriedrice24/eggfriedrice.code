import fc from "fast-check";
import { describe, expect, it } from "vitest";

import { sheetParams } from "./sheet-schemas";
import {
  failedText,
  machineNameOf,
  resultsNotice,
  resultsTitle,
  searchPaneRows,
  sectionsNotice,
  sheetMachines,
  sheetPane,
} from "./sheet-view";
import { item, october } from "./testing";

const NOW = october(26, 14);
const MINUTE = 60_000;
const MACHINES = [
  { id: "d", name: "desktop" },
  { id: "l", name: "laptop" },
];
const machineName = machineNameOf(MACHINES);

const LOADED = [
  [
    item("d", "fan", NOW - 2 * MINUTE, { title: "Why is the fan loud", cwd: "/home/me" }),
    item("d", "zsh", NOW - 9 * MINUTE, { title: "zsh startup is slow", cwd: "/home/me/.dotfiles" }),
  ],
  [item("l", "btrfs", NOW - 5 * MINUTE, { title: "Prune old btrfs snapshots", cwd: "/home/me" })],
];

function keys(rows: readonly { readonly machine: string; readonly id: string }[]): string[] {
  return rows.map((row) => `${row.machine}:${row.id}`);
}

describe("sheetPane", () => {
  it("shows the sections for an empty text and the results for a word", () => {
    expect(sheetPane("")).toBe("sections");
    expect(sheetPane("   ")).toBe("sections");
    expect(sheetPane("fan")).toBe("results");
  });
});

describe("searchPaneRows", () => {
  it("shows the loaded rows that match at once, before efrd answers", () => {
    const rows = searchPaneRows({
      loaded: LOADED,
      found: [],
      text: "laptop",
      searched: "",
      machineName,
      now: NOW,
    });
    expect(keys(rows)).toEqual(["l:btrfs"]);
    expect(rows[0]?.detail).toBe("laptop · ~ · 5m");
  });

  it("puts the hits of efrd first, then the local matches that efrd did not give", () => {
    const rows = searchPaneRows({
      loaded: LOADED,
      found: [
        [item("d", "deep", NOW - 3 * 24 * 60 * MINUTE, { title: "Tune the fan curve" })],
        [item("l", "btrfs", NOW - 5 * MINUTE, { title: "Prune old btrfs snapshots" })],
      ],
      text: "me",
      searched: "me",
      machineName,
      now: NOW,
    });
    // efrd: d:deep and l:btrfs in turn; then the local "me" matches (cwd /home/me).
    expect(keys(rows)).toEqual(["d:deep", "l:btrfs", "d:fan", "d:zsh"]);
  });

  it("leaves out the hits of efrd while they are for an older text", () => {
    const rows = searchPaneRows({
      loaded: LOADED,
      found: [[item("d", "deep", NOW, { title: "Tune the fan curve" })]],
      text: "zsh",
      searched: "fan",
      machineName,
      now: NOW,
    });
    expect(keys(rows)).toEqual(["d:zsh"]);
  });

  it("takes a text with other spaces as the same text", () => {
    const rows = searchPaneRows({
      loaded: [],
      found: [[item("d", "deep", NOW, { title: "Tune the fan curve" })]],
      text: "fan ",
      searched: " fan",
      machineName,
      now: NOW,
    });
    expect(keys(rows)).toEqual(["d:deep"]);
  });

  it("shows each conversation once, whatever the sources hold", () => {
    const arbitraryItem = fc
      .record({
        machine: fc.constantFrom("d", "l"),
        id: fc.constantFrom("a", "b", "c", "d"),
        age: fc.nat({ max: 10_000 }),
      })
      .map((raw) => item(raw.machine, raw.id, NOW - raw.age * MINUTE, { title: `t ${raw.id}` }));
    const lists = fc.array(fc.array(arbitraryItem, { maxLength: 6 }), { maxLength: 3 });
    fc.assert(
      fc.property(lists, lists, (loaded, found) => {
        const rows = searchPaneRows({
          loaded,
          found,
          text: "t",
          searched: "t",
          machineName,
          now: NOW,
        });
        const all = keys(rows);
        expect(new Set(all).size).toBe(all.length);
        const expected = new Set(keys([...loaded.flat(), ...found.flat()]));
        expect(new Set(all)).toEqual(expected);
      }),
    );
  });
});

describe("the notices", () => {
  it("tells the state of the sections pane when it has no rows", () => {
    expect(sectionsNotice({ machines: 0, pending: false, rows: 0 })).toEqual({
      kind: "empty",
      text: "no machine is paired",
    });
    expect(sectionsNotice({ machines: 2, pending: true, rows: 0 })).toEqual({
      kind: "loading",
      text: "loading conversations",
    });
    expect(sectionsNotice({ machines: 2, pending: false, rows: 0 })).toEqual({
      kind: "empty",
      text: "no conversations yet",
    });
    expect(sectionsNotice({ machines: 2, pending: true, rows: 3 })).toEqual({ kind: "none" });
  });

  it("tells the state of the search", () => {
    expect(resultsNotice({ rows: 0, fetching: false, waiting: true })).toEqual({
      kind: "searching",
      text: "searching",
    });
    expect(resultsNotice({ rows: 4, fetching: true, waiting: false }).kind).toBe("searching");
    expect(resultsNotice({ rows: 0, fetching: false, waiting: false })).toEqual({
      kind: "empty",
      text: "no conversations match",
    });
    expect(resultsNotice({ rows: 4, fetching: false, waiting: false })).toEqual({ kind: "none" });
  });

  it("counts the machines that did not answer", () => {
    expect(failedText(0)).toBeUndefined();
    expect(failedText(1)).toBe("1 machine did not answer");
    expect(failedText(3)).toBe("3 machines did not answer");
  });

  it("titles the results with their count", () => {
    expect(resultsTitle(3)).toBe("results · 3");
  });
});

describe("the machines of the sheet", () => {
  it("puts the machine on screen first and keeps the others in order", () => {
    const three = [...MACHINES, { id: "h", name: "homelab" }];
    expect(sheetMachines(three, "h").map((machine) => machine.id)).toEqual(["h", "d", "l"]);
    expect(sheetMachines(three, undefined).map((machine) => machine.id)).toEqual(["d", "l", "h"]);
    expect(sheetMachines(three, "gone").map((machine) => machine.id)).toEqual(["d", "l", "h"]);
  });

  it("names a machine, and a gone machine by the start of its id", () => {
    expect(machineName("l")).toBe("laptop");
    expect(machineName("0123456789abcdef")).toBe("01234567");
  });
});

describe("sheetParams", () => {
  const ID = "019a9b1c-3d00-7a10-8b20-0123456789ab";

  it("keeps the machine of the route", () => {
    expect(sheetParams({ machine: ID })).toEqual({ machine: ID });
    expect(sheetParams({})).toEqual({});
  });

  it("ignores a param that is not a machine id", () => {
    expect(sheetParams({ machine: "desktop" })).toEqual({});
    expect(sheetParams({ machine: [ID, ID] })).toEqual({});
    expect(sheetParams(undefined)).toEqual({});
  });
});
