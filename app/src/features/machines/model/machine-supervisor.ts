// The supervisor of one paired machine (plan section 2.2). It lives outside React, one for
// each machine, and owns:
//
// - the client (src/lib/net/client.ts), which keeps one connection up and resubscribes;
// - the connection atom, which the screens read through `connection`;
// - the streams of the conversations of the machine, made with the factory of the
//   conversation feature, and how long each one stays;
// - the hook that runs on each `ready`, for the invalidation of the machine's queries and
//   the drain of the outbox.
//
// A screen holds a stream with `hold`. A "keep" hold keeps the stream in memory (a screen
// that is in the stack). A "follow" hold also keeps the subscription of the conversation up
// (a screen that has the focus). When the last hold goes, the stream stays for `retainMs`
// more, so that a return to the conversation shows it at once and resumes with `after_seq`.

import { createAtom, type Atom, type Readable } from "@tanstack/store";

import { Client, type ClientOptions, type Subscription } from "@/lib/net/client";
import type { Clock, Timer } from "@/lib/net/clock";
import type { UnaryMethodName } from "@/lib/net/connection";
import { ConnectionError } from "@/lib/net/errors";
import type { SocketFactory } from "@/lib/net/socket";
import type { MethodParams, MethodResults } from "@/lib/protocol/generated";
import type { EventCache } from "@/lib/store/event-cache";
import type { FrameScheduler } from "@/lib/store/frame-store";
import type { PairedDevice, Pairing } from "@/features/pairing/model/paired-device";
import { endpointLabel, endpointOf } from "./machine";
import {
  initialConnection,
  keylessConnection,
  nextConnection,
  statusOf,
  type MachineConnection,
  type MachineStatus,
} from "./machine-connection";
import type { ConversationId, MachineId } from "./machine-ref";
import type { ConversationStream, ConversationStreamFactory, StreamDeps } from "./stream-factory";

/** How long a stream stays after its last hold: 5 minutes (plan section 2.2). */
export const STREAM_RETAIN_MS = 5 * 60_000;

/** The settings of the client that a test can change. */
export type ClientTuning = Partial<
  Pick<ClientOptions, "backoff" | "liveness" | "handshakeTimeoutMs">
>;

/** What a supervisor needs. */
export interface SupervisorDeps {
  readonly sockets: SocketFactory;
  readonly clock: Clock;
  /** A random value in [0, 1) for the backoff ladder. */
  readonly random: () => number;
  /** The `client` member of hello, such as `efr-app 0.0.0`. */
  readonly client: string;
  readonly streams: ConversationStreamFactory;
  readonly cache: EventCache | undefined;
  readonly schedule: FrameScheduler;
  /** Called with errors that the supervisor goes on after. */
  readonly onError: (error: unknown) => void;
  /** Called each time the connection becomes ready. */
  readonly onReady: (machine: MachineId) => void;
  readonly retainMs?: number;
  readonly tuning?: ClientTuning;
}

/** How a hold uses a stream. */
export type HoldMode =
  /** Keep the stream in memory: its screen is in the stack. */
  | "keep"
  /** Keep it, and keep its subscription up: its screen has the focus. */
  | "follow";

/** One hold of a stream. */
export interface StreamHold {
  readonly stream: ConversationStream;
  /** Ends the hold. A second call does nothing. */
  release(): void;
}

/** The options of one call. */
export interface CallOptions {
  /** Stops the wait: the call then rejects with the reason of the signal. */
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
}

interface StreamEntry {
  readonly id: ConversationId;
  readonly stream: ConversationStream;
  keeps: number;
  follows: number;
  hydrated: boolean;
  subscription: Subscription | undefined;
  timer: Timer | undefined;
  gone: boolean;
}

/** The supervisor of one machine. */
export class MachineSupervisor {
  readonly id: MachineId;
  /** The connection to the machine. It changes on each state of the client. */
  readonly connection: Readable<MachineConnection>;
  private readonly atom: Atom<MachineConnection>;
  private readonly deps: SupervisorDeps;
  private readonly streamDeps: StreamDeps;
  private readonly entries = new Map<ConversationId, StreamEntry>();
  private pairing: Pairing;
  private client: Client | undefined;
  private stopListening: (() => void) | undefined;
  private disposed = false;

  constructor(pairing: Pairing, deps: SupervisorDeps) {
    this.id = pairing.device.daemonId;
    this.pairing = pairing;
    this.deps = deps;
    this.streamDeps = {
      cache: deps.cache,
      schedule: deps.schedule,
      clock: deps.clock,
      onError: deps.onError,
    };
    this.atom = createAtom<MachineConnection>(keylessConnection());
    this.connection = this.atom;
    this.attach(pairing, undefined);
  }

  /** The pairing record of the machine. */
  get device(): PairedDevice {
    return this.pairing.device;
  }

  /** Connects. A supervisor without a device key does nothing. */
  start(): void {
    this.client?.start();
  }

  /** The user asks for a try now: start a stopped client, or connect again at once. */
  retry(): void {
    const client = this.client;
    if (client === undefined) {
      return;
    }
    const state = client.state.state;
    if (state === "off" || state === "stopped") {
      client.start();
    } else {
      client.networkChanged();
    }
  }

  /** The app went to the background (`false`) or came back (`true`). */
  setForeground(active: boolean): void {
    this.client?.setForeground(active);
  }

  /** The network path of the phone changed: connect again at once. */
  networkChanged(): void {
    this.client?.networkChanged();
  }

  /**
   * Runs a method on the machine. It rejects with a `ConnectionError` of kind `offline`
   * when the machine is not connected.
   */
  call<M extends UnaryMethodName>(
    method: M,
    params: MethodParams[M],
    options: CallOptions = {},
  ): Promise<MethodResults[M]> {
    const client = this.client;
    if (client === undefined) {
      return Promise.reject(offline("the phone has no device key for this machine"));
    }
    const { signal, timeoutMs } = options;
    if (signal?.aborted === true) {
      return Promise.reject(abortReason(signal));
    }
    const call = client.call(method, params, timeoutMs === undefined ? {} : { timeoutMs });
    if (signal === undefined) {
      return call;
    }
    return untilAborted(call, signal);
  }

  /**
   * What efrd said about itself on the connection that is up now. It throws a
   * `ConnectionError` of kind `offline` when the machine is not connected.
   */
  status(): MachineStatus {
    const status = statusOf(this.atom.get());
    if (status === undefined) {
      throw offline("the phone is not connected to this machine");
    }
    return status;
  }

  /** Holds the stream of conversation `conversationId`; it makes the stream when needed. */
  hold(conversationId: ConversationId, mode: HoldMode): StreamHold {
    if (this.disposed) {
      throw new Error("the machine was forgotten");
    }
    const entry = this.entries.get(conversationId) ?? this.create(conversationId);
    entry.timer?.cancel();
    entry.timer = undefined;
    if (mode === "follow") {
      entry.follows += 1;
      this.subscribe(entry);
    } else {
      entry.keeps += 1;
    }
    let released = false;
    return {
      stream: entry.stream,
      release: () => {
        if (!released) {
          released = true;
          this.release(entry, mode);
        }
      },
    };
  }

  /** The number of streams in memory, for the tests. */
  get streamCount(): number {
    return this.entries.size;
  }

  /**
   * Uses a new pairing of the same machine (a re-pair): a new client with the new device
   * and key. The streams and their holds stay; the followed ones subscribe again.
   */
  replace(pairing: Pairing): void {
    if (this.disposed || pairing.device.daemonId !== this.id) {
      return;
    }
    const seenAt = this.atom.get().seenAt;
    this.detach();
    this.pairing = pairing;
    this.attach(pairing, seenAt);
    this.start();
  }

  /** Closes the connection and drops every stream. The supervisor is of no use after it. */
  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.detach();
    // NOTE: `drop` deletes the entry that the loop is at, which a Map allows.
    for (const entry of this.entries.values()) {
      this.drop(entry);
    }
  }

  private attach(pairing: Pairing, seenAt: number | undefined): void {
    const { key, device } = pairing;
    if (key === null) {
      this.atom.set({ ...keylessConnection(), seenAt });
      return;
    }
    const { deps } = this;
    const options: ClientOptions = {
      endpoint: endpointOf(device),
      key,
      sockets: deps.sockets,
      clock: deps.clock,
      random: deps.random,
      client: deps.client,
      ...deps.tuning,
    };
    const client = new Client(options);
    const endpoint = endpointLabel(device);
    this.client = client;
    this.atom.set({ ...initialConnection(client.state, endpoint), seenAt });
    this.stopListening = client.onState((state) => {
      const previous = this.atom.get();
      this.atom.set(nextConnection(previous, state, endpoint, deps.clock.now()));
      if (state.state === "ready" && previous.client.state !== "ready") {
        try {
          deps.onReady(this.id);
        } catch (error) {
          deps.onError(error);
        }
      }
    });
    for (const entry of this.entries.values()) {
      this.subscribe(entry);
    }
  }

  private detach(): void {
    for (const entry of this.entries.values()) {
      entry.subscription?.close();
      entry.subscription = undefined;
    }
    this.stopListening?.();
    this.stopListening = undefined;
    this.client?.stop();
    this.client = undefined;
  }

  private create(id: ConversationId): StreamEntry {
    const stream = this.deps.streams({ machine: this.id, id }, this.streamDeps);
    const entry: StreamEntry = {
      id,
      stream,
      keeps: 0,
      follows: 0,
      hydrated: false,
      subscription: undefined,
      timer: undefined,
      gone: false,
    };
    this.entries.set(id, entry);
    void this.hydrate(entry);
    return entry;
  }

  /** Reads the cache of a new stream, then starts its subscription when it is followed. */
  private async hydrate(entry: StreamEntry): Promise<void> {
    try {
      await entry.stream.hydrate();
    } catch (error) {
      this.deps.onError(error);
    }
    entry.hydrated = true;
    this.subscribe(entry);
  }

  /** Starts the subscription of a followed stream once its cache is read. */
  private subscribe(entry: StreamEntry): void {
    const client = this.client;
    if (
      client === undefined ||
      entry.gone ||
      !entry.hydrated ||
      entry.follows === 0 ||
      entry.subscription !== undefined
    ) {
      return;
    }
    entry.subscription = client.subscribe(entry.id, entry.stream);
  }

  private release(entry: StreamEntry, mode: HoldMode): void {
    if (mode === "follow") {
      entry.follows -= 1;
      if (entry.follows === 0) {
        entry.subscription?.close();
        entry.subscription = undefined;
      }
    } else {
      entry.keeps -= 1;
    }
    if (entry.gone || entry.keeps + entry.follows > 0) {
      return;
    }
    entry.timer = this.deps.clock.setTimer(this.deps.retainMs ?? STREAM_RETAIN_MS, () => {
      entry.timer = undefined;
      if (entry.keeps + entry.follows === 0) {
        this.drop(entry);
      }
    });
  }

  private drop(entry: StreamEntry): void {
    entry.gone = true;
    entry.timer?.cancel();
    entry.timer = undefined;
    entry.subscription?.close();
    entry.subscription = undefined;
    this.entries.delete(entry.id);
    try {
      entry.stream.dispose();
    } catch (error) {
      this.deps.onError(error);
    }
  }
}

function offline(message: string): ConnectionError {
  return new ConnectionError("offline", message);
}

/** `call`, or a rejection with the reason of `signal` as soon as it aborts. */
async function untilAborted<T>(call: Promise<T>, signal: AbortSignal): Promise<T> {
  let stop: (() => void) | undefined;
  const aborted = new Promise<never>((_, reject) => {
    const abort = (): void => reject(abortReason(signal));
    signal.addEventListener("abort", abort);
    stop = () => signal.removeEventListener("abort", abort);
  });
  try {
    return await Promise.race([call, aborted]);
  } finally {
    stop?.();
  }
}

function abortReason(signal: AbortSignal): unknown {
  const reason: unknown = signal.reason;
  return reason ?? new Error("the call was aborted");
}
