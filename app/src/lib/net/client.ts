// The client: it keeps one connection to the paired efrd up, and keeps the
// subscriptions of the app alive across connections (R3.9 of the research).
//
// - A failed or lost connection waits on the backoff ladder of t3code
//   (src/lib/net/backoff.ts), or longer when efrd asked for a wait (`starting`), then
//   connects again. A connection that stayed up for 30 s resets the ladder.
// - An error that needs the user (pair again, update) stops the client in `stopped`.
// - Back in the foreground after 10 s or more, the client opens a new socket at once,
//   because iOS may have taken the old one's resources. It does not wait for the old
//   socket to close. A change of the network path does the same.
// - Each subscription subscribes again on each new connection, with `after_seq` from its
//   sink, so the stream continues where the app is. An overflow error resubscribes on
//   a backoff ladder of its own, because on a slow link it can repeat.

import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import type {
  ConversationSubscribe,
  ErrorBody,
  HelloResult,
  MethodParams,
  MethodResults,
  ScopeName,
} from "@/lib/protocol/generated";
import type { SubscribeItem } from "@/lib/protocol/wire";
import {
  DEFAULT_BACKOFF,
  INITIAL_BACKOFF,
  backoffConnected,
  backoffFailed,
  type Backoff,
  type BackoffConfig,
} from "./backoff";
import type { Clock, Timer } from "./clock";
import { Connection, type StreamHandle, type UnaryMethodName } from "./connection";
import type { DeviceKey } from "./device-key";
import { socketUrl, type Endpoint } from "./endpoint";
import { ConnectionError } from "./errors";
import { DEFAULT_LIVENESS, replaceOnForeground, type LivenessConfig } from "./liveness";
import type { SocketFactory } from "./socket";

/** The settings of the client. */
export interface ClientOptions {
  readonly endpoint: Endpoint;
  readonly key: DeviceKey;
  readonly sockets: SocketFactory;
  readonly clock: Clock;
  /** A random value in [0, 1) for the backoff ladder. */
  readonly random: () => number;
  /** The `client` member of hello, such as `efr-app 0.0.0`. */
  readonly client: string;
  readonly backoff?: BackoffConfig;
  readonly liveness?: LivenessConfig;
  readonly handshakeTimeoutMs?: number;
}

/** What the client is doing. */
export type ClientState =
  /** `start` was not called yet, or `stop` was. */
  | { readonly state: "off" }
  | { readonly state: "connecting" }
  | {
      readonly state: "ready";
      readonly hello: HelloResult;
      readonly scopes: readonly ScopeName[];
    }
  /** The client waits until `retryAt` (clock time) after `error`. */
  | { readonly state: "waiting"; readonly retryAt: number; readonly error: ConnectionError }
  /** The client stopped after `error`; `error.recovery` says what the user must do. */
  | { readonly state: "stopped"; readonly error: ConnectionError };

/** What a subscription hands its owner, such as a conversation store. */
export interface SubscriptionSink {
  /** The largest sequence number that the sink has, sent as `after_seq`. */
  cursor(): number | undefined;
  /** One item of the stream: decoded, and as it came on the wire (for a cache). */
  item(item: SubscribeItem, raw: unknown): void;
  /** The subscription changed its state. */
  status?(status: SubscriptionStatus): void;
}

/** What a subscription is doing. */
export type SubscriptionStatus =
  /** No connection; it subscribes again when one is ready. */
  | { readonly state: "waiting" }
  | { readonly state: "subscribing" }
  /** The first item came. */
  | { readonly state: "live" }
  /** efrd ended it with an error that can pass, such as `overflow`; it retries at `retryAt`. */
  | { readonly state: "retrying"; readonly retryAt: number; readonly error: ErrorBody }
  /** efrd ended it; it does not subscribe again. */
  | { readonly state: "ended" }
  /** efrd refused it, such as with `not_found`; it does not subscribe again. */
  | { readonly state: "failed"; readonly error: ErrorBody }
  | { readonly state: "closed" };

/** One subscription of the client. */
export interface Subscription {
  readonly conversationId: string;
  /** Ends the subscription. */
  close(): void;
}

/** The error codes after which a subscription subscribes again on its backoff ladder. */
const RETRY_CODES: ReadonlySet<string> = new Set(["overflow", "busy", "cancelled", "internal"]);

interface Entry {
  readonly conversationId: string;
  readonly sink: SubscriptionSink;
  handle: StreamHandle | undefined;
  timer: Timer | undefined;
  backoff: Backoff;
  done: boolean;
}

/** The connection to the paired efrd, kept up. */
export class Client {
  private readonly options: ClientOptions;
  private readonly url: string;
  private readonly entries = new Set<Entry>();
  private readonly listeners = new Set<(state: ClientState) => void>();
  private current: ClientState = { state: "off" };
  private connection: Connection | undefined;
  private generation = 0;
  private backoff: Backoff = INITIAL_BACKOFF;
  private retryTimer: Timer | undefined;
  private backgroundAt: number | undefined;

  constructor(options: ClientOptions) {
    const url = socketUrl(options.endpoint);
    if (url === undefined) {
      throw new Error("the endpoint has no IPv4 address and port");
    }
    this.options = options;
    this.url = url;
  }

  /** What the client is doing now. */
  get state(): ClientState {
    return this.current;
  }

  /** Calls `listener` on each change of the state. Returns the function that stops it. */
  onState(listener: (state: ClientState) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Connects, unless the client runs already. A stopped client starts again. */
  start(): void {
    if (this.current.state === "off" || this.current.state === "stopped") {
      this.backoff = INITIAL_BACKOFF;
      this.connect();
    }
  }

  /** Closes the connection and stops connecting. Subscriptions stay, idle. */
  stop(): void {
    this.generation += 1;
    this.retryTimer?.cancel();
    this.retryTimer = undefined;
    const connection = this.connection;
    this.connection = undefined;
    this.detachAll();
    connection?.close();
    this.setState({ state: "off" });
  }

  /** The app went to the background (`false`) or came back (`true`). */
  setForeground(active: boolean): void {
    const now = this.options.clock.now();
    if (!active) {
      this.backgroundAt ??= now;
      return;
    }
    const replace = replaceOnForeground(
      this.backgroundAt,
      now,
      this.options.liveness ?? DEFAULT_LIVENESS,
    );
    this.backgroundAt = undefined;
    if (this.current.state === "waiting") {
      this.connectNow();
    } else if (replace && (this.current.state === "ready" || this.current.state === "connecting")) {
      this.connectNow();
    }
  }

  /** The network path changed, such as from Wi-Fi to cellular: connect again at once. */
  networkChanged(): void {
    const state = this.current.state;
    if (state === "ready" || state === "connecting" || state === "waiting") {
      this.connectNow();
    }
  }

  /**
   * Runs a method on the ready connection. It rejects with kind `offline` when there is
   * none.
   */
  call<M extends UnaryMethodName>(
    method: M,
    params: MethodParams[M],
    options: { readonly timeoutMs?: number } = {},
  ): Promise<MethodResults[M]> {
    const connection = this.connection;
    if (connection === undefined || this.current.state !== "ready") {
      return Promise.reject(new ConnectionError("offline", "the phone is not connected to efrd"));
    }
    return connection.call(method, params, options);
  }

  /** Follows conversation `conversationId` with drafts, across connections. */
  subscribe(conversationId: string, sink: SubscriptionSink): Subscription {
    const entry: Entry = {
      conversationId,
      sink,
      handle: undefined,
      timer: undefined,
      backoff: INITIAL_BACKOFF,
      done: false,
    };
    this.entries.add(entry);
    if (this.current.state === "ready") {
      this.startEntry(entry);
    } else {
      sink.status?.({ state: "waiting" });
    }
    return {
      conversationId,
      close: () => {
        if (!this.entries.delete(entry)) {
          return;
        }
        entry.done = true;
        entry.timer?.cancel();
        entry.handle?.cancel();
        entry.handle = undefined;
        sink.status?.({ state: "closed" });
      },
    };
  }

  private setState(state: ClientState): void {
    this.current = state;
    for (const listener of this.listeners) {
      listener(state);
    }
  }

  /** Drops the current connection without counting a failure, and connects at once. */
  private connectNow(): void {
    this.retryTimer?.cancel();
    this.retryTimer = undefined;
    const old = this.connection;
    this.connection = undefined;
    this.connect();
    old?.close();
  }

  private connect(): void {
    this.generation += 1;
    const generation = this.generation;
    this.detachAll();
    this.setState({ state: "connecting" });
    const { options } = this;
    const opening = Connection.open({
      url: this.url,
      sockets: options.sockets,
      clock: options.clock,
      client: options.client,
      daemonId: options.endpoint.daemonId,
      device: { id: options.endpoint.deviceId, key: options.key },
      liveness: options.liveness ?? DEFAULT_LIVENESS,
      handshakeTimeoutMs: options.handshakeTimeoutMs ?? 15_000,
      onClose: (error) => {
        if (generation === this.generation) {
          this.connection = undefined;
          this.failed(error);
        }
      },
    });
    opening.then(
      (connection) => this.opened(generation, connection),
      (error: unknown) => {
        if (generation === this.generation) {
          this.failed(
            error instanceof ConnectionError
              ? error
              : new ConnectionError("socket", "the connection failed"),
          );
        }
      },
    );
  }

  private opened(generation: number, connection: Connection): void {
    if (generation !== this.generation) {
      connection.close();
      return;
    }
    this.connection = connection;
    this.backoff = backoffConnected(this.backoff, this.options.clock.now());
    this.setState({ state: "ready", hello: connection.hello, scopes: connection.scopes });
    for (const entry of this.entries) {
      if (entry.handle === undefined && entry.timer === undefined && !entry.done) {
        this.startEntry(entry);
      }
    }
  }

  private failed(error: ConnectionError): void {
    this.detachAll();
    if (error.recovery === "none") {
      return;
    }
    if (error.recovery !== "retry") {
      this.setState({ state: "stopped", error });
      return;
    }
    const now = this.options.clock.now();
    const config = this.options.backoff ?? DEFAULT_BACKOFF;
    const next = backoffFailed(this.backoff, now, this.options.random(), config);
    this.backoff = next.state;
    const delay = Math.max(next.delayMs, error.details.retryAfterMs ?? 0);
    this.retryTimer = this.options.clock.setTimer(delay, () => {
      this.retryTimer = undefined;
      this.connect();
    });
    this.setState({ state: "waiting", retryAt: now + delay, error });
  }

  /** Every subscription loses its stream; it subscribes again on the next connection. */
  private detachAll(): void {
    for (const entry of this.entries) {
      if (entry.handle !== undefined) {
        entry.handle = undefined;
        entry.sink.status?.({ state: "waiting" });
      }
    }
  }

  private startEntry(entry: Entry): void {
    const connection = this.connection;
    if (connection === undefined || entry.done) {
      return;
    }
    const params: ConversationSubscribe = { conversation_id: entry.conversationId, drafts: true };
    const cursor = entry.sink.cursor();
    if (cursor !== undefined && cursor > 0) {
      params.after_seq = cursor;
    }
    let live = false;
    entry.sink.status?.({ state: "subscribing" });
    const handle = connection.stream("conversation.subscribe", params, {
      item: (raw) => {
        if (entry.handle !== handle) {
          return;
        }
        const decoded = decodeSubscribeItem(raw);
        if (!decoded.ok) {
          handle.cancel();
          this.retryEntry(entry, { code: "invalid", message: decoded.reason });
          return;
        }
        if (!live) {
          live = true;
          entry.backoff = backoffConnected(entry.backoff, this.options.clock.now());
          entry.sink.status?.({ state: "live" });
        }
        entry.sink.item(decoded.value, raw);
      },
      end: () => {
        if (entry.handle === handle) {
          entry.handle = undefined;
          entry.done = true;
          entry.sink.status?.({ state: "ended" });
        }
      },
      error: (body) => {
        if (entry.handle !== handle) {
          return;
        }
        if (RETRY_CODES.has(body.code)) {
          this.retryEntry(entry, body);
        } else {
          entry.handle = undefined;
          entry.done = true;
          entry.sink.status?.({ state: "failed", error: body });
        }
      },
      lost: () => {
        if (entry.handle === handle) {
          entry.handle = undefined;
          entry.sink.status?.({ state: "waiting" });
        }
      },
    });
    entry.handle = handle;
  }

  private retryEntry(entry: Entry, error: ErrorBody): void {
    entry.handle = undefined;
    const now = this.options.clock.now();
    const config = this.options.backoff ?? DEFAULT_BACKOFF;
    const next = backoffFailed(entry.backoff, now, this.options.random(), config);
    entry.backoff = next.state;
    entry.timer = this.options.clock.setTimer(next.delayMs, () => {
      entry.timer = undefined;
      if (this.current.state === "ready") {
        this.startEntry(entry);
      } else {
        entry.sink.status?.({ state: "waiting" });
      }
    });
    entry.sink.status?.({ state: "retrying", retryAt: now + next.delayMs, error });
  }
}
