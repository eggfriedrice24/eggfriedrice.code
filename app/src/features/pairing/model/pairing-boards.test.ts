import { describe, expect, it } from "vitest";

import { PAIRING_BOARDS, pairingBoard } from "./pairing-boards";

describe("the pairing boards", () => {
  it("have every pairing board of the canvas (design-spec section 1)", () => {
    const drawn = PAIRING_BOARDS.filter((board) => board.drawn).map((board) => board.name);
    expect(drawn).toEqual([
      "P-Intro",
      "P-Scan",
      "P-Wait",
      "P-Success",
      "P-Errors · code expired",
      "P-Errors · wrong machine",
      "P-Errors · tailscale not connected",
      "P-Errors · refused",
      "P-Errors · efrd too old",
    ]);
  });

  it("have the undrawn states of plan 5.2 item 8.19 for pairing", () => {
    expect(pairingBoard("Pairing · camera denied")?.phase).toEqual({
      kind: "failed",
      problem: { kind: "camera" },
    });
    expect(pairingBoard("Pairing · re-paired")?.phase).toMatchObject({ repaired: true });
  });

  it("have one name for each board", () => {
    const names = PAIRING_BOARDS.map((board) => board.name);
    expect(new Set(names).size).toBe(names.length);
  });
});
