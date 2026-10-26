// The words of the connection states (session.ts `ConnectionView`): what a banner or a
// whole screen says when the phone has no working connection to efrd. On iOS one VPN
// runs at a time, and another VPN turns Tailscale off, so "Tailscale is not connected"
// comes first and in plain words (R5 item 9 of the research). Pure, for the tests.

import type { ConnectionView, PairingFailure, PairingView, RepairReason } from "./session";

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
        body: `efr connects to ${view.endpoint}.`,
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
    default:
      return undefined;
  }
}

/** Why the phone must pair again, in words. */
export function repairText(reason: RepairReason): string {
  switch (reason) {
    case "revoked":
      return "Your computer removed this phone from its devices. To use it again, pair it again with efr device add.";
    case "node_changed":
      return "Tailscale on this phone is a new device now, for example after a new install. efr knows the old one only. Pair the phone again with efr device add.";
    case "unknown_device":
      return "Your computer does not know this phone. Pair it again with efr device add.";
    default:
      return "Your computer does not accept this phone now. Pair it again with efr device add.";
  }
}

/** What the pairing screen says in each state of the flow. */
export interface PairingNotice {
  readonly title: string;
  readonly body: string;
  /** The label of the main button; absent while the flow runs. */
  readonly action?: "Scan the code" | "Try again" | "Open conversations";
  /** The flow runs and can stop. */
  readonly busy: boolean;
}

/** The notice of `view`. */
export function pairingNotice(view: PairingView): PairingNotice {
  switch (view.kind) {
    case "scanning":
      return {
        title: "Scan the code",
        body: "Point the camera at the code that efr device add shows on your computer.",
        busy: true,
      };
    case "connecting":
      return {
        title: "Connecting",
        body: `efr connects to your computer at ${view.host} over Tailscale.`,
        busy: true,
      };
    case "confirm":
      return {
        title: "Confirm on your computer",
        body: `efr device add on your computer now shows this phone as "${view.deviceName}". Confirm it there. The phone waits.`,
        busy: true,
      };
    case "paired":
      return {
        title: "This phone is paired",
        body: "You can follow your conversations and answer approvals from here.",
        action: "Open conversations",
        busy: false,
      };
    case "failed":
      return {
        title: "The phone did not pair",
        body: view.message ?? failureText(view.reason),
        action: "Try again",
        busy: false,
      };
    default:
      return {
        title: "Pair this phone",
        body: "On your computer, run efr device add. It shows a code. Then scan the code here. Your computer and this phone must both be on your Tailscale network.",
        action: "Scan the code",
        busy: false,
      };
  }
}

function failureText(reason: PairingFailure): string {
  switch (reason) {
    case "camera":
      return "efr cannot use the camera. Let efr use it in Settings, then try again.";
    case "not_a_code":
      return "This is not a pairing code of efr. Scan the code that efr device add shows.";
    case "unreachable":
      return "efr cannot reach the computer of the code. Make sure that Tailscale is connected on this phone and that efrd runs.";
    case "wrong_daemon":
      return "The computer that answered is not the one of the code. Run efr device add again and scan the new code.";
    case "expired":
      return "The code expired or was used. Run efr device add again and scan the new code.";
    case "refused":
      return "Your computer did not accept this phone.";
    case "attempts":
      return "There were too many tries with this code. Run efr device add again.";
    default:
      return "Something went wrong. Run efr device add again and scan the new code.";
  }
}
