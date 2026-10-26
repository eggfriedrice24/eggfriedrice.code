// The connection that pairing uses: one WebSocket to efrd's tailnet listener, with the
// requests of the protocol (docs/protocol.md "Summary") and only what pairing needs.
// Before the proof, efrd takes only `hello`, `device.enroll` and `device.prove`. Each of
// them is unary: zero or one `item` frame, then `end` or `error`.
//
// The socket and the timers come in as parameters, so the connection runs in Vitest with a
// fake socket and fake timers. The connection with the full client (the subscribe streams,
// the proof on each reconnect and the liveness rule) is in src/net.

import { decodeServerFrame } from "../core/envelope";
import type { ErrorBody } from "../core/wire";

/** What the connection gets from its socket. */
export interface SocketHandlers {
  open(): void;
  /** One message. efrd sends text messages only. */
  message(data: unknown): void;
  /** The socket closed, or failed; called at most once that matters. */
  close(code: number, reason: string): void;
}

/** A WebSocket, reduced to what the connection uses. */
export interface SocketLike {
  send(text: string): void;
  close(): void;
}

/** Opens a WebSocket to `url` that reports to `handlers`. */
export type OpenSocket = (url: string, handlers: SocketHandlers) => SocketLike;

/** Runs `run` after `ms` milliseconds, unless the returned function is called first. */
export type Schedule = (ms: number, run: () => void) => () => void;

/** A request failed with an error frame of efrd. */
export class RpcError extends Error {
  override readonly name = "RpcError";
  readonly code: string;
  readonly data: unknown;

  constructor(body: ErrorBody) {
    super(body.message);
    this.code = body.code;
    this.data = body.data;
  }
}

/** Why a connection ended or never started. */
export type ConnectionProblem =
  /** No WebSocket came up: the network, Tailscale or efrd is not there. */
  | "unreachable"
  /** The WebSocket closed while a request waited. */
  | "closed"
  /** efrd sent a frame that is not valid. */
  | "protocol";

/** The connection failed, not one request. */
export class ConnectionError extends Error {
  override readonly name = "ConnectionError";
  readonly problem: ConnectionProblem;

  constructor(problem: ConnectionProblem, message: string) {
    super(message);
    this.problem = problem;
  }
}

/** How long the WebSocket may take to come up. */
export const OPEN_TIMEOUT_MS = 10_000;

interface Pending {
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
  item: unknown;
  hasItem: boolean;
}

/** One WebSocket to efrd for the requests of a pairing. */
export class PairingConnection {
  private socket: SocketLike | undefined;
  private readonly pending = new Map<number, Pending>();
  private nextId = 1;
  private ended: ConnectionError | undefined;

  private constructor() {}

  /**
   * Opens a connection to `url`. Fails with a `ConnectionError` of problem `unreachable`
   * when the WebSocket does not come up within `OPEN_TIMEOUT_MS`.
   */
  static open(openSocket: OpenSocket, url: string, schedule: Schedule): Promise<PairingConnection> {
    const connection = new PairingConnection();
    return new Promise((resolve, reject) => {
      let opened = false;
      const stopTimer = schedule(OPEN_TIMEOUT_MS, () => {
        if (!opened) {
          connection.end(new ConnectionError("unreachable", "efrd did not answer in time"));
          reject(new ConnectionError("unreachable", "efrd did not answer in time"));
        }
      });
      connection.socket = openSocket(url, {
        open: () => {
          if (connection.ended !== undefined || opened) {
            return;
          }
          opened = true;
          stopTimer();
          resolve(connection);
        },
        message: (data) => connection.receive(data),
        close: (code, reason) => {
          const problem = opened ? "closed" : "unreachable";
          const error = new ConnectionError(problem, closeText(code, reason));
          connection.end(error);
          if (!opened) {
            stopTimer();
            reject(error);
          }
        },
      });
    });
  }

  /** True after the connection ended. */
  get isClosed(): boolean {
    return this.ended !== undefined;
  }

  /**
   * Sends one unary request. Gives its result, or fails with an `RpcError` for an error
   * frame and a `ConnectionError` when the connection ends first.
   */
  call(method: string, params: unknown): Promise<unknown> {
    if (this.ended !== undefined) {
      return Promise.reject(this.ended);
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject, item: null, hasItem: false });
      this.socket?.send(JSON.stringify({ id, method, params }));
    });
  }

  /** Closes the connection. Each request that waits fails with problem `closed`. */
  close(): void {
    this.end(new ConnectionError("closed", "the app closed the connection"));
  }

  private receive(data: unknown): void {
    if (this.ended !== undefined) {
      return;
    }
    if (typeof data !== "string") {
      this.end(new ConnectionError("protocol", "efrd sent a message that is not text"));
      return;
    }
    const decoded = decodeServerFrame(data);
    if (!decoded.ok) {
      this.end(
        new ConnectionError("protocol", `efrd sent a frame that is not valid: ${decoded.reason}`),
      );
      return;
    }
    const frame = decoded.value;
    if (frame.type === "error" && frame.id === null) {
      // An error without an id ends the connection: efrd could not read a frame.
      this.end(new ConnectionError("protocol", frame.error.message));
      return;
    }
    const id = frame.id;
    const pending = id === null ? undefined : this.pending.get(id);
    if (id === null || pending === undefined) {
      return;
    }
    switch (frame.type) {
      case "item":
        if (pending.hasItem) {
          this.pending.delete(id);
          pending.reject(new ConnectionError("protocol", "efrd sent two results for one request"));
          return;
        }
        pending.item = frame.item;
        pending.hasItem = true;
        return;
      case "end":
        this.pending.delete(id);
        if (pending.hasItem) {
          pending.resolve(pending.item);
        } else {
          pending.reject(new ConnectionError("protocol", "efrd ended a request without a result"));
        }
        return;
      case "error":
        this.pending.delete(id);
        pending.reject(new RpcError(frame.error));
        return;
      case "ack":
        return;
    }
  }

  private end(error: ConnectionError): void {
    if (this.ended !== undefined) {
      return;
    }
    this.ended = error;
    for (const pending of this.pending.values()) {
      pending.reject(error);
    }
    this.pending.clear();
    this.socket?.close();
  }
}

function closeText(code: number, reason: string): string {
  return reason === ""
    ? `the connection closed (code ${code})`
    : `the connection closed (code ${code}): ${reason}`;
}
