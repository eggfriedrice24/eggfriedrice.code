import { describe, expect, it } from "vitest";

import { pairingFailure } from "./pairing-view";

describe("pairingFailure", () => {
  it("names each failure of the flow for the screen", () => {
    expect(pairingFailure({ kind: "cancelled" })).toEqual({ kind: "idle" });
    expect(pairingFailure({ kind: "not_efr" })).toEqual({ kind: "failed", reason: "not_a_code" });
    expect(pairingFailure({ kind: "protocol_mismatch" })).toEqual({
      kind: "failed",
      reason: "protocol",
    });
    expect(pairingFailure({ kind: "wrong_daemon" })).toEqual({
      kind: "failed",
      reason: "wrong_daemon",
    });
    expect(
      pairingFailure({
        kind: "other_machine",
        daemonId: "019a9b1c-3d00-7a10-8b20-0000000000aa",
        host: "100.64.0.9",
        fqdn: "homelab.tail1234.ts.net",
      }),
    ).toEqual({ kind: "failed", reason: "other_machine", machine: "homelab" });
    expect(
      pairingFailure({
        kind: "other_machine",
        daemonId: "019a9b1c-3d00-7a10-8b20-0000000000aa",
        host: "100.64.0.9",
      }),
    ).toEqual({ kind: "failed", reason: "other_machine", machine: "100.64.0.9" });
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
