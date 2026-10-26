// One connection to efrd's tailnet listener: the socket, hello, the proof of the device
// key, the request table and the liveness rule.
//
// The order on a new socket (R3.6 and R3.7 of the research):
//
// 1. `hello` with origin `phone`. The result must name the expected daemon and protocol;
//    else the connection fails before anything else is sent.
// 2. With a device key: sign the prove message of the result's challenge and facts
//    (src/lib/protocol/prove.ts) and call `device.prove`. Without one (while pairing) the
//    connection is ready after hello, and the caller sends `device.enroll`.
// 3. Requests and streams. Each request has an id, unique on the connection.
//
// The liveness rule (src/lib/net/liveness.ts): JS never sees the WebSocket pongs, so the
// connection counts only the frames that it decodes. After a silence it sends one
// probe: a second `hello`. The transport of efrd answers a second hello itself with a
// `conflict` error, before the proof and after it, and runs nothing; so any live efrd
// answers the probe with a frame. After a longer silence the connection is dead and
// closes, and the client connects again.

import { base64Encode } from "@/lib/protocol/bytes";
import { decodeServerFrame, isJsonObject } from "@/lib/protocol/envelope";
import {
  PROTOCOL_VERSION,
  type ErrorBody,
  type Hello,
  type HelloResult,
  type MethodName,
  type MethodParams,
  type MethodResults,
  type ScopeName,
  type StreamMethodName,
} from "@/lib/protocol/generated";
import { proveFields, proveMessage } from "@/lib/protocol/prove";
import type { Clock, Timer } from "./clock";
import type { DeviceKey } from "./device-key";
import { ConnectionError, ServerError, asksToRepair, retryAfterMs } from "./errors";
import {
  DEFAULT_LIVENESS,
  liveness,
  nextLivenessCheckAt,
  type LivenessConfig,
  type LivenessState,
} from "./liveness";
import { CLOSE_NORMAL, CLOSE_SILENT, type Socket, type SocketFactory } from "./socket";

/** A method that answers with one result. */
export type UnaryMethodName = Exclude<MethodName, StreamMethodName>;

/** The enrolled device and its key. */
export interface Device {
  readonly id: string;
  readonly key: DeviceKey;
}

/** The settings of one connection. */
export interface ConnectionOptions {
  /** The URL of the listener (src/lib/net/endpoint.ts). */
  readonly url: string;
  readonly sockets: SocketFactory;
  readonly clock: Clock;
  /** The `client` member of hello, such as `efr-app 0.0.0`. */
  readonly client: string;
  /** The daemon that must answer; another daemon fails with `daemon_changed`. */
  readonly daemonId: string;
  /** The enrolled device. Absent while the phone pairs. */
  readonly device?: Device;
  readonly liveness?: LivenessConfig;
  /** The longest time from the socket's start to ready. */
  readonly handshakeTimeoutMs?: number;
  /** Called once when a ready connection closes, with the reason. */
  readonly onClose?: (error: ConnectionError) => void;
}

/** The longest time from the socket's start to ready, by default. */
export const DEFAULT_HANDSHAKE_TIMEOUT_MS = 15_000;

/** What a stream request hands its caller. */
export interface StreamHandlers {
  /** One item, not yet decoded. */
  item(item: unknown): void;
  /** The daemon ended the stream. */
  end(): void;
  /** The daemon ended the stream with an error. */
  error(body: ErrorBody): void;
  /** The connection closed before the stream ended. */
  lost(error: ConnectionError): void;
}

/** A running stream request. */
export interface StreamHandle {
  readonly id: number;
  /** Asks efrd to cancel the stream. No handler runs after this. */
  cancel(): void;
}

type Pending =
  | {
      readonly kind: "unary";
      result: { readonly value: unknown } | undefined;
      readonly resolve: (value: unknown) => void;
      readonly reject: (error: Error) => void;
      readonly timer: Timer | undefined;
    }
  | { readonly kind: "stream"; readonly handlers: StreamHandlers }
  | { readonly kind: "probe" };

type Phase = "opening" | "handshake" | "ready" | "closed";

/** One connection. `Connection.open` gives a ready one. */
export class Connection {
  private readonly options: ConnectionOptions;
  private readonly pending = new Map<number, Pending>();
  private readonly livenessConfig: LivenessConfig;
  private readonly opened: Promise<void>;
  private markOpened: () => void = () => {};
  private socket: Socket | undefined;
  private phase: Phase = "opening";
  private nextId = 1;
  private livenessState: LivenessState;
  private livenessTimer: Timer | undefined;
  private closeError: ConnectionError | undefined;
  private helloResult: HelloResult | undefined;
  private grantedScopes: readonly ScopeName[] = [];
  /** Rejects when the connection fails before it is ready. */
  private readonly failedEarly: Promise<never>;
  private rejectEarly: (error: ConnectionError) => void = () => {};

  private constructor(options: ConnectionOptions) {
    this.options = options;
    this.livenessConfig = options.liveness ?? DEFAULT_LIVENESS;
    this.livenessState = { lastFrameAt: options.clock.now(), probeSentAt: undefined };
    this.opened = new Promise((resolve) => {
      this.markOpened = resolve;
    });
    this.failedEarly = new Promise((_, reject) => {
      this.rejectEarly = reject;
    });
  }

  /**
   * Opens a connection and resolves when it is ready: after the proof, or after hello
   * when `options.device` is absent. It rejects with a `ConnectionError`.
   */
  static async open(options: ConnectionOptions): Promise<Connection> {
    const connection = new Connection(options);
    const timer = options.clock.setTimer(
      options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS,
      () => connection.fail(new ConnectionError("timeout", "efrd did not answer in time")),
    );
    try {
      await Promise.race([connection.handshake(), connection.failedEarly]);
    } catch (error) {
      connection.fail(
        error instanceof ConnectionError
          ? error
          : new ConnectionError("socket", "the connection failed"),
      );
      throw connection.closeError ?? error;
    } finally {
      timer.cancel();
    }
    connection.becomeReady();
    return connection;
  }

  /** The hello result of efrd. */
  get hello(): HelloResult {
    if (this.helloResult === undefined) {
      throw new Error("the connection has no hello result yet");
    }
    return this.helloResult;
  }

  /** The scopes that the proof granted; empty while the phone pairs. */
  get scopes(): readonly ScopeName[] {
    return this.grantedScopes;
  }

  /** True once the connection closed. */
  get closed(): boolean {
    return this.phase === "closed";
  }

  /**
   * Runs a method with one result. It rejects with a `ServerError` when efrd answers
   * with an error, and with a `ConnectionError` when the connection closes first or
   * `timeoutMs` passes (the request is then cancelled).
   */
  async call<M extends UnaryMethodName>(
    method: M,
    params: MethodParams[M],
    options: { readonly timeoutMs?: number } = {},
  ): Promise<MethodResults[M]> {
    return trustedResult(method, await this.unary(method, params, options.timeoutMs));
  }

  /** Starts a stream method. Its frames go to `handlers` until it ends or is cancelled. */
  stream<M extends StreamMethodName>(
    method: M,
    params: MethodParams[M],
    handlers: StreamHandlers,
  ): StreamHandle {
    const id = this.takeId();
    const handle: StreamHandle = {
      id,
      cancel: () => {
        if (this.pending.get(id)?.kind === "stream") {
          this.pending.delete(id);
          this.sendText(JSON.stringify({ cancel: id }));
        }
      },
    };
    const error = this.closeError;
    if (error !== undefined) {
      void Promise.resolve().then(() => handlers.lost(error));
      return handle;
    }
    this.pending.set(id, { kind: "stream", handlers });
    this.sendText(JSON.stringify({ id, method, params }));
    return handle;
  }

  /** Closes the connection. Every pending request fails with kind `closed`. */
  close(): void {
    this.fail(new ConnectionError("closed", "the app closed the connection"));
  }

  private async handshake(): Promise<void> {
    let socket: Socket;
    try {
      socket = this.options.sockets(this.options.url, {
        opened: () => this.markOpened(),
        message: (data) => this.received(data),
        closed: (code) =>
          this.fail(
            new ConnectionError("socket", `the socket closed (${code})`, { closeCode: code }),
          ),
      });
    } catch {
      throw new ConnectionError("socket", "the socket could not open");
    }
    this.socket = socket;
    await this.opened;
    this.phase = "handshake";

    const params: Hello = {
      protocol: PROTOCOL_VERSION,
      origin: "phone",
      client: this.options.client,
    };
    const device = this.options.device;
    if (device !== undefined) {
      params.device_id = device.id;
    }
    const hello = helloResult(await this.handshakeStep("hello", params));
    if (hello === undefined) {
      throw new ConnectionError("malformed", "the hello result is malformed");
    }
    if (hello.protocol !== PROTOCOL_VERSION) {
      throw new ConnectionError(
        "protocol_mismatch",
        `efrd speaks protocol ${hello.protocol}, the app ${PROTOCOL_VERSION}`,
      );
    }
    if (hello.daemon_id !== this.options.daemonId) {
      throw new ConnectionError("daemon_changed", "another daemon answered at this address");
    }
    this.helloResult = hello;
    if (device !== undefined) {
      await this.prove(device);
    }
  }

  /**
   * Proves the key of `device` on this connection and returns the scopes that efrd
   * granted. `open` does it when it has a device; the pairing calls it after
   * `device.enroll` on the same connection. It rejects with a `ConnectionError`, such as
   * `proof_refused`; efrd closes the connection after a few failed proofs.
   */
  async prove(device: Device): Promise<readonly ScopeName[]> {
    const fields = proveFields(this.hello, device.id);
    if (fields === undefined) {
      throw new ConnectionError("no_prove_context", "the hello result has no facts to prove");
    }
    let signature: Uint8Array;
    try {
      signature = await device.key.sign(proveMessage(fields));
    } catch {
      throw new ConnectionError("key_unavailable", "the device key could not sign");
    }
    const proof: MethodParams["device.prove"] = {
      device_id: device.id,
      signature: base64Encode(signature),
    };
    this.grantedScopes = scopesOf(await this.handshakeStep("device.prove", proof));
    return this.grantedScopes;
  }

  /** One request of the handshake; an error answer becomes a `ConnectionError`. */
  private async handshakeStep(method: "hello" | "device.prove", params: unknown): Promise<unknown> {
    try {
      return await this.unary(method, params, undefined);
    } catch (error) {
      throw error instanceof ServerError ? handshakeError(method, error.body) : error;
    }
  }

  private becomeReady(): void {
    this.phase = "ready";
    this.livenessState = { lastFrameAt: this.options.clock.now(), probeSentAt: undefined };
    this.scheduleLiveness();
  }

  private unary(method: string, params: unknown, timeoutMs: number | undefined): Promise<unknown> {
    const closed = this.closeError;
    if (closed !== undefined) {
      return Promise.reject(closed);
    }
    const id = this.takeId();
    return new Promise((resolve, reject) => {
      const timer =
        timeoutMs === undefined
          ? undefined
          : this.options.clock.setTimer(timeoutMs, () => {
              if (this.pending.delete(id)) {
                this.sendText(JSON.stringify({ cancel: id }));
                reject(new ConnectionError("timeout", `${method} did not answer in time`));
              }
            });
      this.pending.set(id, { kind: "unary", result: undefined, resolve, reject, timer });
      this.sendText(JSON.stringify({ id, method, params }));
    });
  }

  private takeId(): number {
    const id = this.nextId;
    this.nextId += 1;
    return id;
  }

  private sendText(text: string): void {
    if (this.phase === "closed" || this.socket === undefined) {
      return;
    }
    try {
      this.socket.send(text);
    } catch {
      this.fail(new ConnectionError("socket", "the socket could not send"));
    }
  }

  private scheduleLiveness(): void {
    this.livenessTimer?.cancel();
    const at = nextLivenessCheckAt(this.livenessState, this.livenessConfig);
    const delay = Math.max(0, at - this.options.clock.now());
    this.livenessTimer = this.options.clock.setTimer(delay, () => this.checkLiveness());
  }

  private checkLiveness(): void {
    if (this.phase !== "ready") {
      return;
    }
    const now = this.options.clock.now();
    const verdict = liveness(this.livenessState, now, this.livenessConfig);
    if (verdict === "dead") {
      this.fail(new ConnectionError("silent", "efrd sent nothing for too long"), CLOSE_SILENT);
      return;
    }
    if (verdict === "probe") {
      const id = this.takeId();
      this.pending.set(id, { kind: "probe" });
      const params: Hello = {
        protocol: PROTOCOL_VERSION,
        origin: "phone",
        client: this.options.client,
      };
      this.sendText(JSON.stringify({ id, method: "hello", params }));
      this.livenessState = { ...this.livenessState, probeSentAt: now };
    }
    this.scheduleLiveness();
  }

  private received(data: unknown): void {
    if (this.phase === "closed") {
      return;
    }
    if (typeof data !== "string") {
      this.fail(new ConnectionError("malformed", "efrd sent a binary message"));
      return;
    }
    const decoded = decodeServerFrame(data);
    if (!decoded.ok) {
      this.fail(new ConnectionError("malformed", `efrd sent a bad frame: ${decoded.reason}`));
      return;
    }
    // A frame answers any probe, also one sent in the same millisecond.
    this.livenessState = { lastFrameAt: this.options.clock.now(), probeSentAt: undefined };
    const frame = decoded.value;
    if (frame.type === "error" && frame.id === null) {
      // An error without an id: efrd closes the connection after it.
      this.fail(new ConnectionError("refused", frame.error.message, { body: frame.error }));
      return;
    }
    const id = frame.id;
    const pending = id === null ? undefined : this.pending.get(id);
    if (id === null || pending === undefined) {
      // A frame of a request that the app cancelled or that timed out.
      return;
    }
    switch (frame.type) {
      case "item":
        if (pending.kind === "unary") {
          // A unary method sends one item; anything after the first is ignored.
          pending.result ??= { value: frame.item };
        } else if (pending.kind === "stream") {
          pending.handlers.item(frame.item);
        }
        return;
      case "ack":
        return;
      case "end":
        this.pending.delete(id);
        if (pending.kind === "unary") {
          pending.timer?.cancel();
          if (pending.result === undefined) {
            pending.reject(new ConnectionError("malformed", "efrd ended a request with no result"));
          } else {
            pending.resolve(pending.result.value);
          }
        } else if (pending.kind === "stream") {
          pending.handlers.end();
        }
        return;
      case "error":
        this.pending.delete(id);
        if (pending.kind === "unary") {
          pending.timer?.cancel();
          pending.reject(new ServerError(frame.error));
        } else if (pending.kind === "stream") {
          pending.handlers.error(frame.error);
        }
        return;
    }
  }

  private fail(error: ConnectionError, code = CLOSE_NORMAL): void {
    if (this.phase === "closed") {
      return;
    }
    const wasReady = this.phase === "ready";
    this.phase = "closed";
    this.closeError = error;
    this.livenessTimer?.cancel();
    try {
      this.socket?.close(code);
    } catch {
      // The socket is closed already.
    }
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const request of pending) {
      if (request.kind === "unary") {
        request.timer?.cancel();
        request.reject(error);
      } else if (request.kind === "stream") {
        request.handlers.lost(error);
      }
    }
    if (wasReady) {
      this.options.onClose?.(error);
    } else {
      this.rejectEarly(error);
    }
  }
}

/** The connection error for an error answer to a step of the handshake. */
function handshakeError(step: "hello" | "device.prove", body: ErrorBody): ConnectionError {
  const details = asksToRepair(body) ? { body, rePair: true } : { body };
  if (body.code === "protocol_mismatch") {
    return new ConnectionError("protocol_mismatch", body.message, details);
  }
  if (body.code === "busy") {
    const wait = retryAfterMs(body);
    return new ConnectionError(
      "starting",
      body.message,
      wait === undefined ? details : { ...details, retryAfterMs: wait },
    );
  }
  if (step === "device.prove" && (body.code === "unauthorized" || body.code === "forbidden")) {
    return new ConnectionError("proof_refused", body.message, details);
  }
  return new ConnectionError("refused", body.message, details);
}

/** The hello result, after a check of the members that the connection reads. */
function helloResult(value: unknown): HelloResult | undefined {
  if (!isJsonObject(value)) {
    return undefined;
  }
  const { daemon_id: daemonId, protocol, challenge, prove } = value;
  if (
    typeof daemonId !== "string" ||
    typeof protocol !== "number" ||
    typeof challenge !== "string"
  ) {
    return undefined;
  }
  if (
    prove !== undefined &&
    prove !== null &&
    (!isJsonObject(prove) ||
      typeof prove["peer_node"] !== "string" ||
      typeof prove["daemon_node"] !== "string" ||
      typeof prove["daemon_tailnet"] !== "string")
  ) {
    return undefined;
  }
  return trustedResult("hello", value);
}

function scopesOf(value: unknown): readonly ScopeName[] {
  if (!isJsonObject(value) || !Array.isArray(value["scopes"])) {
    return [];
  }
  return trustedResult("device.prove", value).scopes;
}

/**
 * The result of `method` as its generated type. efrd is trusted for the shape of a
 * result (R6 item 10 of the research); a caller that reads a member checks it.
 */
function trustedResult<M extends MethodName>(_method: M, value: unknown): MethodResults[M] {
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- efrd is trusted for results.
  return value as MethodResults[M];
}
