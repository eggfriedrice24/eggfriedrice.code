import { describe, expect, it } from "vitest";

import { pairingFailure } from "./pairing-view";

describe("pairingFailure", () => {
  it("names each failure of the flow for the screen", () => {
    expect(pairingFailure({ kind: "cancelled" })).toEqual({ kind: "idle" });
    expect(pairingFailure({ kind: "not_efr" })).toEqual({ kind: "failed", reason: "not_a_code" });
    expect(pairingFailure({ kind: "protocol_mismatch" })).toEqual({
      kind: "failed",
      reason: "wrong_daemon",
    });
    expect(pairingFailure({ kind: "token_refused", message: "no" })).toEqual({
      kind: "failed",
      reason: "expired",
    });
    expect(pairingFailure({ kind: "failed", message: "the store failed" })).toEqual({
      kind: "failed",
      reason: "other",
      message: "The pairing failed: the store failed.",
    });
  });
});
