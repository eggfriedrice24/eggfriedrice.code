// What the screens show about the pairing flow. `pairingFailure` maps a failure of the
// flow (./pair.ts) to the view that the pairing screens read. Pure, for the tests.

import type { PairingFailure as FlowFailure } from "./pair";
import { machineName } from "./paired-device";

/** The state of the pairing flow. */
export type PairingView =
  | { readonly kind: "idle" }
  /** The system's code scanner is open. */
  | { readonly kind: "scanning" }
  /** The phone connects to the computer of the code. */
  | { readonly kind: "connecting"; readonly host: string }
  /** The phone sent its key; the user must confirm on the computer. */
  | { readonly kind: "confirm"; readonly host: string; readonly deviceName: string }
  | { readonly kind: "paired" }
  | {
      readonly kind: "failed";
      readonly reason: PairingFailure;
      readonly message?: string;
      /** For `other_machine`: the short name of the machine of the code, such as `homelab`. */
      readonly machine?: string;
    };

/** Why the pairing failed. */
export type PairingFailure =
  /** The user did not let efr use the camera. */
  | "camera"
  /** The code is not a pairing code of efr. */
  | "not_a_code"
  /** The computer of the code does not answer. */
  | "unreachable"
  /** The computer answered with another daemon id or protocol than the code says. */
  | "wrong_daemon"
  /** A re-pair of one machine got the code of another machine (`machine`). */
  | "other_machine"
  /** The code expired, or it was used. */
  | "expired"
  /** The user refused the phone on the computer. */
  | "refused"
  /** Too many tries with the code. */
  | "attempts"
  | "other";

/** The view after a pairing that failed. */
export function pairingFailure(failure: FlowFailure): PairingView {
  switch (failure.kind) {
    case "cancelled":
      return { kind: "idle" };
    case "not_efr":
    case "malformed":
      return failed("not_a_code");
    case "code_version":
      return failed("not_a_code", "This code is for a newer version of the app. Update the app.");
    case "unreachable":
      return failed("unreachable");
    case "wrong_daemon":
    case "protocol_mismatch":
      return failed("wrong_daemon");
    case "other_machine":
      return { kind: "failed", reason: "other_machine", machine: machineName(failure) };
    case "token_refused":
      return failed("expired");
    case "refused":
      return failed("refused");
    case "key_failed":
      return failed("other", `The phone could not make its device key: ${failure.message}.`);
    default:
      // `failed`: the flow names the cause.
      return failed("other", `The pairing failed: ${failure.message}.`);
  }
}

/** A failed pairing view. */
function failed(reason: PairingFailure, message?: string): PairingView {
  return message === undefined ? { kind: "failed", reason } : { kind: "failed", reason, message };
}
