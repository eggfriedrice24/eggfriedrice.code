import { describe, expect, it } from "vitest";

import { GALLERY_BOARDS, SECTIONS, boardsOf, galleryBoard } from "./boards";
import { STAGES } from "./demo-stages";

/** The 30 boards of the design canvas (design-spec section 1), by their file names. */
const CANVAS = [
  "Main",
  "R-Streaming",
  "R-Approval",
  "R-Other",
  "R-History",
  "R-NoMachine",
  "R-MachineMenu",
  "R-TitleMenu",
  "R-AllSheet",
  "C-Offline",
  "C-Reconnecting",
  "C-Failed",
  "C-Jump",
  "Settings",
  "Settings-Menu",
  "Settings-Forget",
  "Settings-ForgetLast",
  "P-Intro",
  "P-Scan",
  "P-Wait",
  "P-Success",
  "P-Errors",
  "S-Composer",
  "S-Approvals",
  "S-Messages",
  "S-Markdown",
  "S-Tools",
  "S-Diff",
  "S-Loaders",
  "Icon",
];

/** The five errors that the P-Errors board draws. */
const PAIRING_ERRORS = [
  "code expired",
  "wrong machine",
  "tailscale not connected",
  "refused",
  "efrd too old",
];

/** The undrawn states of plan 5.2 item 8.19, by the gallery name of each. */
const UNDRAWN = [
  "Strip · 2 approvals waiting",
  "Settings · no machine",
  "Pairing · camera denied",
  "Conversation · re-pair needed",
  "Conversation · efrd is starting",
  "Pairing · re-paired",
  "Conversation · loading",
  "Root · title menu",
];

describe("the inventory of the gallery", () => {
  it("has every board of the canvas once", () => {
    expect(CANVAS).toHaveLength(30);
    const drawn = GALLERY_BOARDS.filter((board) => board.drawn).map(
      (board) => board.name.split(" · ")[0] ?? "",
    );
    expect([...new Set(drawn)].sort()).toEqual([...CANVAS].sort());
    for (const name of CANVAS.filter((one) => one !== "P-Errors")) {
      expect(drawn.filter((one) => one === name)).toHaveLength(1);
    }
  });

  it("has each of the five errors of P-Errors", () => {
    expect(
      GALLERY_BOARDS.filter((board) => board.name.startsWith("P-Errors · ")).map((board) =>
        board.name.slice("P-Errors · ".length),
      ),
    ).toEqual(PAIRING_ERRORS);
  });

  it("has every undrawn state of item 8.19", () => {
    for (const name of UNDRAWN) {
      const board = galleryBoard(name);
      expect(board?.drawn).toBe(false);
      expect(board?.section).toBe("undrawn");
    }
  });

  it("names each board once and puts it in a section", () => {
    const names = GALLERY_BOARDS.map((board) => board.name);
    expect(new Set(names).size).toBe(names.length);
    const sections = new Set(SECTIONS.map(([section]) => section));
    for (const board of GALLERY_BOARDS) {
      expect(sections.has(board.section)).toBe(true);
      expect(board.title).not.toBe("");
    }
    expect(SECTIONS.reduce((sum, [section]) => sum + boardsOf(section).length, 0)).toBe(
      GALLERY_BOARDS.length,
    );
  });

  it("opens only conversations that its stage has", () => {
    for (const board of GALLERY_BOARDS) {
      const visit = board.visit;
      if (visit.kind !== "conversation") {
        continue;
      }
      const keys = new Set<string>(STAGES[visit.stage].conversations.map((one) => one.key));
      for (const key of visit.open) {
        expect(keys.has(key), `${board.name} opens ${key}`).toBe(true);
      }
    }
  });

  it("changes a machine only after a conversation that its stage has", () => {
    for (const stage of Object.values(STAGES)) {
      const keys = new Set<string>(stage.conversations.map((one) => one.key));
      for (const change of "changes" in stage ? stage.changes : []) {
        expect(keys.has(change.after), `${stage.name} waits for ${change.after}`).toBe(true);
      }
    }
  });
});
