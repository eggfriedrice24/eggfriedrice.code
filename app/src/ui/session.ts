// The session: everything the screens read from and send to efrd, as one interface.
// The app's store and network client (src/store, src/net) and the pairing flow
// (src/pairing) implement it; the screens know only this file. Each value that changes
// is an ExternalStore, so a screen reads it with useSyncExternalStore and renders only
// when the part that it selects changes.
//
// A conversation's transcript is two stores, fed from the same subscribe stream:
// `stream` is the stream reducer of the core (src/core/reducer.ts) and changes on each
// draft; `details` is the details reducer (src/ui/details.ts) and changes on events. The
// store runs both reducers on each item.

import type { StreamState } from "../core/reducer";
import type { PhoneDecision } from "./approval";
import type { TranscriptDetails } from "./details";
import type { ExternalStore } from "./external_store";

/** Why the phone has no working connection to efrd. */
export type ConnectionView =
  /** No device key and endpoint yet: the phone must pair first. */
  | { readonly kind: "unpaired" }
  /** A socket opens, or the proof runs. */
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
  | { readonly kind: "repair_needed"; readonly reason: RepairReason };

/** Why the phone must pair again. */
export type RepairReason = "revoked" | "node_changed" | "unknown_device" | "other";

/** One conversation of the list (from efr-protocol's `ConversationSummary`). */
export interface ConversationItem {
  readonly id: string;
  readonly title?: string;
  /** `idle`, `running` or `awaiting_approval`; another value is shown as idle. */
  readonly status: string;
  /** When its last event was recorded, in ms since the epoch. */
  readonly updatedAt: number;
  /** The working directory at the last prompt. */
  readonly cwd?: string;
}

/** The stores of one open conversation. */
export interface ConversationSource {
  readonly id: string;
  readonly stream: ExternalStore<StreamState>;
  readonly details: ExternalStore<TranscriptDetails>;
  /** The title of the conversation, when it has one. */
  readonly title: ExternalStore<string | undefined>;
}

/** An open conversation: its stores, and a way to stop following it. */
export interface OpenConversation {
  readonly source: ConversationSource;
  /** Ends the subscription. The stores keep their last values. */
  readonly close: () => void;
}

/** The result of an action that efrd must accept. */
export type ActionResult =
  | { readonly ok: true }
  | {
      readonly ok: false;
      /** One sentence for the user. */
      readonly message: string;
    };

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
  | { readonly kind: "failed"; readonly reason: PairingFailure; readonly message?: string };

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
  /** The code expired, or it was used. */
  | "expired"
  /** The user refused the phone on the computer. */
  | "refused"
  /** Too many tries with the code. */
  | "attempts"
  | "other";

/** The pairing flow (src/pairing). */
export interface PairingFlow {
  readonly state: ExternalStore<PairingView>;
  /** Opens the scanner and runs the flow to its end. */
  readonly start: () => void;
  /** Stops the flow and goes back to `idle`. */
  readonly cancel: () => void;
}

/** Everything the screens use. */
export interface PhoneSession {
  readonly connection: ExternalStore<ConnectionView>;
  readonly conversations: ExternalStore<readonly ConversationItem[]>;
  readonly pairing: PairingFlow;
  /** Subscribes to a conversation, with drafts. */
  readonly open: (conversationId: string) => OpenConversation;
  /** Sends a prompt to a conversation (`prompt.send`). */
  readonly sendPrompt: (conversationId: string, text: string) => Promise<ActionResult>;
  /** Answers an approval question (`approval.respond`). */
  readonly answerApproval: (
    conversationId: string,
    callId: string,
    decision: PhoneDecision,
  ) => Promise<ActionResult>;
  /** Tries to connect now, without waiting for the next retry. */
  readonly reconnect: () => void;
  /** Forgets the device key and the endpoint, so the phone can pair again. */
  readonly forget: () => Promise<void>;
}
