// Why a connection to efrd failed or closed, and what the app does about it.

import type { ErrorBody } from "@/lib/protocol/generated";

/** Why a connection failed or closed. */
export type ConnectionErrorKind =
  /** The socket failed or closed. */
  | "socket"
  /** The socket did not open, or hello or the proof did not answer, in time. */
  | "timeout"
  /** No frame came for too long (the liveness rule). */
  | "silent"
  /** efrd sent a frame that the envelope guard refused. */
  | "malformed"
  /** efrd is starting and cannot take the connection yet; retry after `retryAfterMs`. */
  | "starting"
  /** efrd speaks another protocol version. The app or efrd must be updated. */
  | "protocol_mismatch"
  /** Another daemon answered at the address of the paired one. */
  | "daemon_changed"
  /** The hello result has no prove facts: the address is not efrd's tailnet listener. */
  | "no_prove_context"
  /**
   * efrd refused the proof. With `details.rePair` the device paired from another
   * Tailscale node; else efrd does not know the device, or it was revoked (efrd sends
   * one message for every such cause).
   */
  | "proof_refused"
  /** The device key cannot sign, for example because the app was installed again. */
  | "key_unavailable"
  /** efrd refused hello or the proof with another error. */
  | "refused"
  /** The client has no ready connection now. */
  | "offline"
  /** The app closed the connection. */
  | "closed";

/** What the client does after an error of each kind. */
export type Recovery =
  /** Connect again after the backoff wait. */
  | "retry"
  /** Stop: the phone must pair again (`efr device add`). */
  | "repair"
  /** Stop: the app or efrd must be updated. */
  | "update"
  /** Nothing: the app closed the connection on purpose. */
  | "none";

const RECOVERY: Readonly<Record<ConnectionErrorKind, Recovery>> = {
  socket: "retry",
  timeout: "retry",
  silent: "retry",
  malformed: "retry",
  starting: "retry",
  no_prove_context: "retry",
  refused: "retry",
  offline: "retry",
  protocol_mismatch: "update",
  daemon_changed: "repair",
  proof_refused: "repair",
  key_unavailable: "repair",
  closed: "none",
};

/** Details of a connection error. */
export interface ConnectionErrorDetails {
  /** The error that efrd sent, when it sent one. */
  readonly body?: ErrorBody;
  /** The wait that efrd asked for, for `starting`. */
  readonly retryAfterMs?: number;
  /** The close code of the socket, for `socket`. */
  readonly closeCode?: number;
  /**
   * True when the error data has `re_pair: true`: the device paired from another
   * Tailscale node, so the phone must pair again (docs/protocol.md "Remote devices").
   */
  readonly rePair?: boolean;
}

/** A failed or closed connection. */
export class ConnectionError extends Error {
  readonly kind: ConnectionErrorKind;
  readonly details: ConnectionErrorDetails;

  constructor(kind: ConnectionErrorKind, message: string, details: ConnectionErrorDetails = {}) {
    super(message);
    this.name = "ConnectionError";
    this.kind = kind;
    this.details = details;
  }

  /** What the client does next. */
  get recovery(): Recovery {
    return RECOVERY[this.kind];
  }
}

/** A request that efrd answered with an error. */
export class ServerError extends Error {
  readonly body: ErrorBody;

  constructor(body: ErrorBody) {
    super(body.message);
    this.name = "ServerError";
    this.body = body;
  }
}

/**
 * The wait in milliseconds that the data of a `busy` error asks for: `retry_after_ms`,
 * or `retry_after_secs` times 1000. `undefined` when the data names none.
 */
export function retryAfterMs(body: ErrorBody): number | undefined {
  const data = body.data;
  if (data === undefined) {
    return undefined;
  }
  const ms = data["retry_after_ms"];
  if (typeof ms === "number" && Number.isFinite(ms) && ms >= 0) {
    return ms;
  }
  const secs = data["retry_after_secs"];
  if (typeof secs === "number" && Number.isFinite(secs) && secs >= 0) {
    return secs * 1000;
  }
  return undefined;
}

/** True when the error data says `re_pair: true`. */
export function asksToRepair(body: ErrorBody): boolean {
  return body.data?.["re_pair"] === true;
}
