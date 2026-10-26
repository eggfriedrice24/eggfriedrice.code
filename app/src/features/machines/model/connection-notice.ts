// The words of the connection states (`ConnectionView` of ./connection-view.ts): what a
// banner or a whole screen says when the phone has no working connection to efrd. On iOS
// one VPN runs at a time, and another VPN turns Tailscale off, so "Tailscale is not
// connected" comes first and in plain words (R5 item 9 of the research). The words of the
// pairing screens are in src/features/pairing/model/pairing-view.ts. Pure, for the tests.

import type { ConnectionView, RepairReason } from "./connection-view";

/** What the screens say about the connection. */
export interface ConnectionNotice {
  readonly title: string;
  readonly body: string;
  /** Seconds until the next try, rounded up; absent when no try is planned. */
  readonly retryIn?: number;
  /** The user can ask for a try now. */
  readonly canRetry: boolean;
  readonly tone: "muted" | "warning" | "error";
}

/** The notice of `view` at `now` (ms since the epoch), or `undefined` when online. */
export function connectionNotice(view: ConnectionView, now: number): ConnectionNotice | undefined {
  const retry = (at: number | undefined) =>
    at === undefined ? {} : { retryIn: Math.max(0, Math.ceil((at - now) / 1000)) };
  switch (view.kind) {
    case "online":
      return undefined;
    case "unpaired":
      return {
        title: "This phone is not paired",
        body: "To pair it, run efr device add on your computer and scan the code.",
        canRetry: false,
        tone: "muted",
      };
    case "connecting":
      return {
        title: "Connecting to your computer",
        body:
          view.endpoint === ""
            ? "efr connects to your computer."
            : `efr connects to ${view.endpoint}.`,
        canRetry: false,
        tone: "muted",
      };
    case "unreachable":
      return {
        title: "Tailscale is not connected",
        body: `efr cannot reach your computer at ${view.endpoint}. Turn on Tailscale on this phone. If it is on, make sure that the computer is awake and that efrd runs.`,
        ...retry(view.retryAt),
        canRetry: true,
        tone: "warning",
      };
    case "starting":
      return {
        title: "efrd is starting",
        body: "efrd on your computer is not ready yet. The phone tries again soon.",
        ...retry(view.retryAt),
        canRetry: true,
        tone: "muted",
      };
    case "retrying":
      return {
        title: "The connection stopped",
        body: "The phone connects again soon.",
        ...retry(view.retryAt),
        canRetry: true,
        tone: "muted",
      };
    case "repair_needed":
      return {
        title: "Pair this phone again",
        body: repairText(view.reason),
        canRetry: false,
        tone: "error",
      };
    case "update_needed":
      return {
        title: "Update efr",
        body: "This app and efrd on your computer speak different versions of the protocol. Update the app or efr, then open the app again.",
        canRetry: false,
        tone: "error",
      };
    default:
      return undefined;
  }
}

/** Why the phone must pair again, in words. */
export function repairText(reason: RepairReason): string {
  switch (reason) {
    case "node_changed":
      return "Tailscale on this phone is a new device now, for example after a new install. efr knows the old one only. Pair the phone again with efr device add.";
    case "unknown_device":
      return "Your computer does not accept this phone. It removed the phone from its devices, or it does not know it. To use the phone again, pair it with efr device add.";
    default:
      return "Your computer does not accept this phone now. Pair it again with efr device add.";
  }
}
