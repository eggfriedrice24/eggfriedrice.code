// The session of a real phone: the PhoneSession of the screens (src/ui/session.ts) over
// the client (src/net), the stores (src/store) and the pairing flow (src/pairing).
//
// - `start` reads the pairing record and its device key. A paired phone then connects:
//   the client proves the key on each connection, and the stores load the conversation
//   list on each one. A phone without a record is `unpaired`.
// - The pairing flow scans the code, pairs, and then connects with the new device, so
//   the screens find the phone online when the flow says `paired`.
// - The connection view follows the client: a wait after a failed connection shows
//   "Tailscale is not connected", "efrd is starting" or "The connection stopped", and an
//   error that needs the user shows "Pair this phone again" or "Update efr".
// - A conversation's stores follow the conversation while a screen listens to one of
//   them, and stop when the last listener goes.
//
// Every effect comes in as a parameter (src/session/native.ts gives the real ones), so
// the session runs in Vitest with a fake socket, a fake clock and fake key stores.

import { EMPTY_STREAM, type StreamState } from "../core/reducer";
import { Client, type ClientState } from "../net/client";
import type { Clock } from "../net/clock";
import type { Endpoint } from "../net/endpoint";
import { ConnectionError, ServerError } from "../net/errors";
import type { SocketFactory } from "../net/socket";
import { cancellation, pair, type PairingDeps, type PairingFailure } from "../pairing/pair";
import { forgetPairing, loadPairing, type Pairing } from "../pairing/paired_device";
import { scanPairingCode, type ScannerApi } from "../pairing/scanner";
import type { ConversationSummary } from "../protocol/generated";
import { AppStore, type HeldConversation } from "../store/app_store";
import { ApprovalRefused } from "../store/conversation";
import type { EventCache } from "../store/event_cache";
import type { PhoneDecision } from "../ui/approval";
import { EMPTY_DETAILS, reduceDetails, type TranscriptDetails } from "../ui/details";
import { createValueStore, type ExternalStore, type ValueStore } from "../ui/external_store";
import type {
  ActionResult,
  ConnectionView,
  ConversationItem,
  ConversationSource,
  PairingView,
  PhoneSession,
  RepairReason,
  PairingFailure as ViewFailure,
} from "../ui/session";

/** The effects of the session. */
export interface PhoneSessionDeps {
  /** The effects of the pairing flow; null when the build has no device key module. */
  readonly pairing: PairingDeps | null;
  readonly scanner: ScannerApi;
  /** The name that the phone suggests for itself, such as "Anna's iPhone". */
  readonly deviceName: () => string;
  readonly sockets: SocketFactory;
  /** The clock of the client. Its `now` is the wall clock: the screens count to `retryAt`. */
  readonly clock: Clock;
  /** A random value in [0, 1) for the backoff ladder. */
  readonly random: () => number;
  /** The `client` member of hello, such as `efr-app 0.0.0`. */
  readonly client: string;
  /** Opens the event cache once; without it the stores keep nothing. */
  readonly openCache?: () => Promise<EventCache>;
  /** A new command id (a v4 UUID) for each write. */
  readonly newCommandId: () => string;
  /**
   * Follows what the client must hear of the app, such as the foreground (AppState).
   * Returns the function that stops it.
   */
  readonly follow?: (client: Client) => () => void;
  /** Called with errors that the session goes on after, such as a failed cache. */
  readonly onError?: (error: unknown) => void;
}

/** The session, and the functions that start and stop it. */
export interface LivePhoneSession extends PhoneSession {
  /** Reads the pairing and connects when the phone is paired. Call it again after `dispose`. */
  readonly start: () => Promise<void>;
  /** Closes the connection and stops every timer. */
  readonly dispose: () => void;
}

/** The client and the stores of the paired device. */
interface Active {
  readonly client: Client;
  readonly store: AppStore;
  readonly endpoint: string;
  readonly stop: () => void;
}

/** A session over `deps`. */
export function createPhoneSession(deps: PhoneSessionDeps): LivePhoneSession {
  const connection = createValueStore<ConnectionView>({ kind: "connecting", endpoint: "" });
  const conversations = createValueStore<readonly ConversationItem[]>([]);
  const pairingState = createValueStore<PairingView>({ kind: "idle" });
  const sources = new Map<string, LiveSource>();
  let active: Active | undefined;
  let cache: Promise<EventCache | undefined> | undefined;
  let generation = 0;
  let cancelPairing: (() => void) | undefined;

  const openCache = (): Promise<EventCache | undefined> => {
    cache ??= (deps.openCache?.() ?? Promise.resolve(undefined)).catch((error: unknown) => {
      deps.onError?.(error);
      return undefined;
    });
    return cache;
  };

  const deactivate = (): void => {
    const old = active;
    active = undefined;
    for (const source of sources.values()) {
      source.detach();
    }
    old?.stop();
    conversations.set([]);
  };

  const activate = (pairing: Pairing, eventCache: EventCache | undefined): void => {
    deactivate();
    const { device, key } = pairing;
    const endpoint: Endpoint = {
      daemonId: device.daemonId,
      host: device.host,
      port: device.port,
      ...(device.fqdn === undefined ? {} : { fqdn: device.fqdn }),
      deviceId: device.deviceId,
    };
    const label = `${device.host}:${device.port}`;
    const client = new Client({
      endpoint,
      key,
      sockets: deps.sockets,
      clock: deps.clock,
      random: deps.random,
      client: deps.client,
    });
    const store = new AppStore({
      client,
      ...(eventCache === undefined ? {} : { cache: eventCache }),
      onCacheError: (error) => deps.onError?.(error),
      onItem: (conversationId, item) => sources.get(conversationId)?.item(item),
    });
    const stopState = client.onState((state) => connection.set(connectionView(state, label)));
    const stopList = store.conversations.subscribe(() =>
      conversations.set(store.conversations.getState().conversations.map(conversationItem)),
    );
    const stopFollow = deps.follow?.(client);
    active = {
      client,
      store,
      endpoint: label,
      stop: () => {
        stopFollow?.();
        stopList();
        stopState();
        store.dispose();
        client.stop();
      },
    };
    for (const source of sources.values()) {
      source.attach(store);
    }
    client.start();
  };

  const start = async (): Promise<void> => {
    generation += 1;
    const run = generation;
    const keys = deps.pairing?.keys;
    const devices = deps.pairing?.devices;
    let pairing: Pairing | null = null;
    if (keys !== undefined && devices !== undefined) {
      try {
        pairing = await loadPairing(keys, devices);
      } catch (error) {
        deps.onError?.(error);
      }
    }
    if (pairing === null) {
      if (run === generation) {
        connection.set({ kind: "unpaired" });
      }
      return;
    }
    const eventCache = await openCache();
    if (run === generation) {
      activate(pairing, eventCache);
    }
  };

  const startPairing = (): void => {
    const pairingDeps = deps.pairing;
    if (pairingDeps === null) {
      pairingState.set({
        kind: "failed",
        reason: "other",
        message: "This build of the app has no device key, so it cannot pair.",
      });
      return;
    }
    cancelPairing?.();
    const { signal, cancel } = cancellation();
    cancelPairing = cancel;
    const flow = async (): Promise<void> => {
      pairingState.set({ kind: "scanning" });
      const scanned = await scanPairingCode(deps.scanner, signal);
      if (!scanned.ok) {
        pairingState.set(scanFailure(scanned.reason));
        return;
      }
      let host = "";
      const result = await pair(scanned.text, pairingDeps, {
        name: deps.deviceName(),
        signal,
        onStep: (step) => {
          if (step.step === "connecting") {
            host = hostOf(step.url);
            pairingState.set({ kind: "connecting", host });
          } else if (step.step === "confirm") {
            pairingState.set({ kind: "confirm", host, deviceName: step.name });
          }
        },
      });
      if (!result.ok) {
        pairingState.set(pairingFailure(result.failure));
        return;
      }
      const key = await pairingDeps.keys.load(result.device.keyAlias).catch(() => null);
      if (key === null) {
        pairingState.set({
          kind: "failed",
          reason: "other",
          message: "The phone paired, but it cannot read its new device key.",
        });
        return;
      }
      generation += 1;
      activate({ device: result.device, key }, await openCache());
      pairingState.set({ kind: "paired" });
    };
    flow()
      .catch((error: unknown) => {
        deps.onError?.(error);
        pairingState.set({ kind: "failed", reason: "other" });
      })
      .finally(() => {
        if (cancelPairing === cancel) {
          cancelPairing = undefined;
        }
      });
  };

  const conversation = (conversationId: string): ConversationSource => {
    let source = sources.get(conversationId);
    if (source === undefined) {
      source = new LiveSource(conversationId, conversations);
      sources.set(conversationId, source);
      if (active !== undefined) {
        source.attach(active.store);
      }
    }
    return source.view;
  };

  const sendPrompt = async (conversationId: string, text: string): Promise<ActionResult> => {
    const client = active?.client;
    if (client === undefined) {
      return NOT_PAIRED;
    }
    try {
      await client.call("prompt.send", {
        command_id: deps.newCommandId(),
        conversation_id: conversationId,
        text,
      });
      return OK;
    } catch (error) {
      return refused(error);
    }
  };

  const answerApproval = async (
    conversationId: string,
    callId: string,
    decision: PhoneDecision,
  ): Promise<ActionResult> => {
    const now = active;
    if (now === undefined) {
      return NOT_PAIRED;
    }
    const held = now.store.hold(conversationId);
    try {
      await held.store.respond(now.client, callId, decision, deps.newCommandId());
      return OK;
    } catch (error) {
      return refused(error);
    } finally {
      held.release();
    }
  };

  const reconnect = (): void => {
    const client = active?.client;
    if (client === undefined) {
      return;
    }
    const state = client.state.state;
    if (state === "off" || state === "stopped") {
      client.start();
    } else {
      client.networkChanged();
    }
  };

  const forget = async (): Promise<void> => {
    generation += 1;
    cancelPairing?.();
    deactivate();
    connection.set({ kind: "unpaired" });
    pairingState.set({ kind: "idle" });
    const keys = deps.pairing?.keys;
    const devices = deps.pairing?.devices;
    if (keys !== undefined && devices !== undefined) {
      await forgetPairing(keys, devices);
    }
  };

  return {
    connection,
    conversations,
    pairing: {
      state: pairingState,
      start: startPairing,
      cancel: () => {
        cancelPairing?.();
        pairingState.set({ kind: "idle" });
      },
    },
    conversation,
    sendPrompt,
    answerApproval,
    reconnect,
    forget,
    start,
    dispose: () => {
      generation += 1;
      cancelPairing?.();
      deactivate();
    },
  };
}

/**
 * The stores of one conversation for the screens. They hold the conversation's store of
 * the app while any of them has a listener, so a screen follows the conversation exactly
 * while it shows it.
 */
class LiveSource {
  readonly view: ConversationSource;
  private readonly stream = createValueStore<StreamState>(EMPTY_STREAM);
  private readonly details = createValueStore<TranscriptDetails>(EMPTY_DETAILS);
  private readonly title = createValueStore<string | undefined>(undefined);
  private readonly list: ExternalStore<readonly ConversationItem[]>;
  private store: AppStore | undefined;
  private held: HeldConversation | undefined;
  private stopHeld: (() => void) | undefined;
  private stopList: (() => void) | undefined;
  private listeners = 0;

  constructor(
    private readonly conversationId: string,
    list: ExternalStore<readonly ConversationItem[]>,
  ) {
    this.list = list;
    this.view = {
      id: conversationId,
      stream: this.counted(this.stream),
      details: this.counted(this.details),
      title: this.counted(this.title),
    };
  }

  /** Follows the conversation in `store` from now on, while a screen listens. */
  attach(store: AppStore): void {
    this.detach();
    this.store = store;
    if (this.listeners > 0) {
      this.hold();
    }
  }

  /** Stops following the conversation. The stores keep what they show. */
  detach(): void {
    this.release();
    this.store = undefined;
  }

  /** One item that the conversation's store applied, as JSON. */
  item(item: unknown): void {
    this.details.update((state) => reduceDetails(state, item));
  }

  private counted<T>(store: ValueStore<T>): ExternalStore<T> {
    return {
      getSnapshot: store.getSnapshot,
      subscribe: (listener) => {
        const off = store.subscribe(listener);
        this.listeners += 1;
        if (this.listeners === 1) {
          this.hold();
        }
        let done = false;
        return () => {
          if (done) {
            return;
          }
          done = true;
          off();
          this.listeners -= 1;
          if (this.listeners === 0) {
            this.release();
          }
        };
      },
    };
  }

  private hold(): void {
    const store = this.store;
    if (store === undefined || this.held !== undefined) {
      return;
    }
    const held = store.hold(this.conversationId);
    this.held = held;
    const sync = (): void => {
      const view = held.store.getState();
      this.stream.set(view.stream);
      const fromList = this.list.getSnapshot().find((item) => item.id === this.conversationId);
      this.title.set(view.summary?.title ?? fromList?.title ?? undefined);
    };
    this.stopHeld = held.store.subscribe(sync);
    this.stopList = this.list.subscribe(sync);
    sync();
  }

  private release(): void {
    this.stopHeld?.();
    this.stopList?.();
    this.stopHeld = undefined;
    this.stopList = undefined;
    this.held?.release();
    this.held = undefined;
  }
}

const OK: ActionResult = { ok: true };

const NOT_PAIRED: ActionResult = { ok: false, message: "The phone is not paired." };

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

/** One row of the conversation list. */
export function conversationItem(summary: ConversationSummary): ConversationItem {
  const updatedAt = Date.parse(summary.updated_at);
  return {
    id: summary.id,
    ...(summary.title === undefined || summary.title === null ? {} : { title: summary.title }),
    status: summary.status,
    updatedAt: Number.isNaN(updatedAt) ? 0 : updatedAt,
    ...(summary.cwd === undefined || summary.cwd === null ? {} : { cwd: summary.cwd }),
  };
}

/** The view after a scan that gave no code. */
function scanFailure(reason: "unavailable" | "permission" | "cancelled"): PairingView {
  switch (reason) {
    case "permission":
      return { kind: "failed", reason: "camera" };
    case "unavailable":
      return {
        kind: "failed",
        reason: "other",
        message: "This phone has no code scanner. efr needs iOS 16 or later and a camera.",
      };
    default:
      return { kind: "idle" };
  }
}

/** The view after a pairing that failed. */
export function pairingFailure(failure: PairingFailure): PairingView {
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
function failed(reason: ViewFailure, message?: string): PairingView {
  return message === undefined ? { kind: "failed", reason } : { kind: "failed", reason, message };
}

/** The result of an action that failed with `error`. */
function refused(error: unknown): ActionResult {
  if (error instanceof ApprovalRefused) {
    return { ok: false, message: APPROVAL_REFUSALS[error.reason] };
  }
  if (error instanceof ServerError) {
    return { ok: false, message: sentence(error.body.message) };
  }
  if (error instanceof ConnectionError) {
    return { ok: false, message: "The phone is not connected to your computer. Try again." };
  }
  return { ok: false, message: "Something went wrong. Try again." };
}

const APPROVAL_REFUSALS: Readonly<Record<ApprovalRefused["reason"], string>> = {
  unknown: "The phone does not know this question yet.",
  not_pending: "This question has an answer, or it expired.",
  local_only: "Only your computer can answer this question.",
  decision: "The phone cannot give this answer.",
};

/** `text` as a sentence: a capital letter at the start and a full stop at the end. */
function sentence(text: string): string {
  const trimmed = text.trim();
  if (trimmed === "") {
    return "Your computer refused it.";
  }
  const first = trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return /[.!?]$/.test(first) ? first : `${first}.`;
}

/** The host of a `ws://host:port/` URL. */
function hostOf(url: string): string {
  const match = /^ws:\/\/([^:/]+)/.exec(url);
  return match?.[1] ?? url;
}
