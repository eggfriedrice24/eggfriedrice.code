// What the screens show about the connection of the phone to one machine (efrd). The
// client (src/lib/net/client.ts) gives a `ClientState`; `connectionView` maps it to the
// view that the screens read. Pure, for the tests.

import type { ClientState } from "@/lib/net/client";
import type { ConnectionError } from "@/lib/net/errors";

/** Why the phone has no working connection to efrd. */
export type ConnectionView =
  /** No device key and endpoint yet: the phone must pair first. */
  | { readonly kind: "unpaired" }
  /**
   * A socket opens, or the proof runs. `endpoint` is empty while the app still reads
   * its pairing.
   */
  | { readonly kind: "connecting"; readonly endpoint: string }
  /** Proven and subscribed. */
  | { readonly kind: "online"; readonly endpoint: string; readonly daemonName?: string }
  /**
   * The efrd address does not answer. On a phone this nearly always means that Tailscale
   * is off, or that another VPN took its place; it can also mean that the computer
   * sleeps or efrd does not run. The client tries again at `retryAt`.
   */
  | { readonly kind: "unreachable"; readonly endpoint: string; readonly retryAt?: number }
  /** efrd answered that it is starting: the client tries again at `retryAt`. */
  | { readonly kind: "starting"; readonly endpoint: string; readonly retryAt?: number }
  /** The connection closed; the client tries again at `retryAt`. */
  | { readonly kind: "retrying"; readonly endpoint: string; readonly retryAt?: number }
  /**
   * efrd does not accept this phone any more: the device was revoked, or the phone's
   * Tailscale node changed (a new install), so the device key no longer matches.
   */
  | { readonly kind: "repair_needed"; readonly reason: RepairReason }
  /** The app and efrd speak different versions of the protocol. */
  | { readonly kind: "update_needed" };

/** Why the phone must pair again. */
export type RepairReason =
  /** The phone's Tailscale node is not the one that paired (efrd's `re_pair`). */
  | "node_changed"
  /**
   * efrd refused the proof: it does not know the device, or the device was revoked.
   * efrd gives the same answer for both, so the app cannot tell them apart.
   */
  | "unknown_device"
  /** Another daemon answers at the address, or the device key is gone. */
  | "other";

/** What the screens show for the state of the client. `endpoint` is `host:port`. */
export function connectionView(state: ClientState, endpoint: string): ConnectionView {
  switch (state.state) {
    case "off":
    case "connecting":
      return { kind: "connecting", endpoint };
    case "ready":
      return { kind: "online", endpoint };
    case "waiting": {
      const { retryAt, error } = state;
      if (error.kind === "starting") {
        return { kind: "starting", endpoint, retryAt };
      }
      if (error.kind === "socket" || error.kind === "timeout") {
        return { kind: "unreachable", endpoint, retryAt };
      }
      return { kind: "retrying", endpoint, retryAt };
    }
    default:
      // `stopped`: the error needs the user.
      return state.error.recovery === "update"
        ? { kind: "update_needed" }
        : { kind: "repair_needed", reason: repairReason(state.error) };
  }
}

/** Why the phone must pair again after `error`. */
export function repairReason(error: ConnectionError): RepairReason {
  if (error.kind === "proof_refused") {
    return error.details.rePair === true ? "node_changed" : "unknown_device";
  }
  return "other";
}
