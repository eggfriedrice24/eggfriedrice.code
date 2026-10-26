// The pairing flow (R3.6 of the research, docs/protocol.md "Remote devices"):
//
// 1. Parse the QR code.
// 2. Connect to efrd's tailnet address and send `hello` (src/lib/net/connection.ts, without a
//    device). The connection stops before the token goes out when the `daemon_id` or the
//    `protocol` of the result is not the one of the code.
// 3. Make a new device key and send `device.enroll` with the token, the device name and
//    the public key. efrd shows the phone's facts at the terminal; the request waits until
//    the user answers there.
// 4. Keep the pairing record, and remove the key of an earlier pairing with the same
//    daemon (a re-pair). The records of the other machines stay.
//
// Every effect (the socket, the clock, the key store, the record store, SHA-256, time)
// comes in as a parameter, so the flow runs in Vitest. src/features/pairing/native/native-pairing.ts gives the
// real ones.

import type { Clock } from "@/lib/net/clock";
import { Connection } from "@/lib/net/connection";
import { ConnectionError, ServerError } from "@/lib/net/errors";
import type { SocketFactory } from "@/lib/net/socket";
import { base64Encode } from "@/lib/protocol/bytes";
import { isJsonObject } from "@/lib/protocol/envelope";
import { DeviceKeyError, keyFingerprint } from "./device-key";
import type { DeviceKey, DeviceKeyStore } from "./device-key";
import { deviceName } from "./device-name";
import { daemonUrl, stringArray } from "./paired-device";
import type { PairedDevice, PairedDeviceStore } from "./paired-device";
import { parsePairingUrl } from "./pairing-url";
import type { PairingCode } from "./pairing-url";

/** The effects that the flow uses. */
export interface PairingDeps {
  readonly sockets: SocketFactory;
  readonly clock: Clock;
  readonly keys: DeviceKeyStore;
  readonly devices: PairedDeviceStore;
  /** The SHA-256 of `data`. */
  readonly sha256: (data: Uint8Array) => Promise<Uint8Array>;
  /** A fresh alias for a new device key, such as `device-<uuid>`. */
  readonly newKeyAlias: () => string;
  /** The current time, for the record. */
  readonly now: () => Date;
  /** The client program for `hello`, such as `efr-app 0.0.0`. */
  readonly client: string;
}

/** Where the flow is, for the screen. */
export type PairingStep =
  /** The phone connects and checks the daemon with `hello`. */
  | { readonly step: "connecting"; readonly url: string }
  /** The terminal shows the facts of the phone; the user must confirm there. */
  | { readonly step: "confirm"; readonly name: string; readonly fingerprint: string }
  | { readonly step: "saving" };

/** Why a pairing did not give a device. */
export type PairingFailure =
  /** The QR code is not an efr pairing code. */
  | { readonly kind: "not_efr" }
  /** The code is for a later version of the app. */
  | { readonly kind: "code_version" }
  /** The code is an efr code, but it is damaged. */
  | { readonly kind: "malformed" }
  /**
   * A re-pair of one machine got the code of another machine. The flow stops before it
   * connects, so the token does not go out.
   */
  | {
      readonly kind: "other_machine";
      /** The daemon of the code. */
      readonly daemonId: string;
      readonly host: string;
      /** The MagicDNS name of the code, when it has one. */
      readonly fqdn?: string;
    }
  /** No connection came up: Tailscale is off, or efrd does not listen. */
  | { readonly kind: "unreachable" }
  /** The computer at the address is not the one of the code. The token did not go out. */
  | { readonly kind: "wrong_daemon" }
  /**
   * efrd speaks another protocol version. The token did not go out. `protocol` and
   * `version` are efrd's, when it named them.
   */
  | { readonly kind: "protocol_mismatch"; readonly protocol?: number; readonly version?: string }
  /** efrd refused the token: it is wrong, used or expired. Run `efr device add` again. */
  | { readonly kind: "token_refused"; readonly message: string }
  /** The user said no at the terminal, or the pairing expired before the answer. */
  | { readonly kind: "refused"; readonly message: string }
  /** The phone could not make or use its device key. */
  | { readonly kind: "key_failed"; readonly message: string }
  /** The flow was cancelled. */
  | { readonly kind: "cancelled" }
  /** Another failure, with a message for the user. */
  | { readonly kind: "failed"; readonly message: string };

/** The result of a pairing. */
export type PairingResult =
  | { readonly ok: true; readonly device: PairedDevice }
  | { readonly ok: false; readonly failure: PairingFailure };

/**
 * A pairing that did not give a device, as an error: the pairing mutation rejects with it,
 * so that `isError` and `onSuccess` of the mutation mean what they say.
 */
export class PairingError extends Error {
  readonly failure: PairingFailure;

  constructor(failure: PairingFailure) {
    super(`the pairing failed: ${failure.kind}`);
    this.name = "PairingError";
    this.failure = failure;
  }
}

/** A signal that the screen sets to stop the flow. */
export interface CancelSignal {
  readonly cancelled: boolean;
  /** Calls `listener` once on cancel. Gives a function that removes it. */
  onCancel(listener: () => void): () => void;
}

/** A signal and the function that sets it. */
export function cancellation(): { readonly signal: CancelSignal; readonly cancel: () => void } {
  const listeners = new Set<() => void>();
  let cancelled = false;
  return {
    signal: {
      get cancelled() {
        return cancelled;
      },
      onCancel(listener) {
        if (cancelled) {
          listener();
          return () => {};
        }
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    },
    cancel: () => {
      if (!cancelled) {
        cancelled = true;
        for (const listener of listeners) {
          listener();
        }
        listeners.clear();
      }
    },
  };
}

/** The options of one pairing. */
export interface PairingOptions {
  /** The name that efrd shows; cleaned with `deviceName`. */
  readonly name: string;
  /**
   * The daemon id of the machine that the user pairs again. A code of another daemon
   * then fails with `other_machine`. Absent for the pairing of a new machine.
   */
  readonly repair?: string;
  readonly signal?: CancelSignal;
  readonly onStep?: (step: PairingStep) => void;
}

class Cancelled extends Error {}

/** Pairs the phone with the daemon of the QR code `text`. */
export async function pair(
  text: string,
  deps: PairingDeps,
  options: PairingOptions,
): Promise<PairingResult> {
  const parsed = parsePairingUrl(text);
  if (!parsed.ok) {
    const kind = parsed.reason === "version" ? "code_version" : parsed.reason;
    return { ok: false, failure: { kind } };
  }
  const { code } = parsed;
  if (options.repair !== undefined && options.repair !== code.daemonId) {
    const failure = { kind: "other_machine", daemonId: code.daemonId, host: code.host } as const;
    return {
      ok: false,
      failure: code.fqdn === undefined ? failure : { ...failure, fqdn: code.fqdn },
    };
  }
  const run = new PairingRun(code, deps, options);
  try {
    return { ok: true, device: await run.run() };
  } catch (error) {
    await run.cleanUp();
    return { ok: false, failure: failureOf(error, options.signal) };
  } finally {
    run.stop();
  }
}

/** The state of one pairing, so that a failure can clean up what the run made. */
class PairingRun {
  private connection: Connection | undefined;
  /** The alias of the key that this run makes, until the record names it. */
  private newAlias: string | undefined;
  private creating: Promise<DeviceKey> | undefined;
  private stopped = false;
  private readonly removeCancel: () => void;
  private readonly cancelled: Promise<never>;

  constructor(
    private readonly code: PairingCode,
    private readonly deps: PairingDeps,
    private readonly options: PairingOptions,
  ) {
    const cancelled = rejection();
    this.cancelled = cancelled.promise;
    this.removeCancel =
      options.signal?.onCancel(() => {
        cancelled.reject(new Cancelled());
        this.connection?.close();
      }) ?? (() => {});
  }

  async run(): Promise<PairedDevice> {
    const connection = await this.connect();

    const name = deviceName(this.options.name);
    const alias = this.deps.newKeyAlias();
    this.newAlias = alias;
    this.creating = this.deps.keys.create(alias).catch(keyError);
    const key = await this.until(this.creating);
    const fingerprint = keyFingerprint(await this.until(this.deps.sha256(key.publicKey)));
    this.step({ step: "confirm", name, fingerprint });
    const enrolled = await this.until(
      connection.call("device.enroll", {
        token: this.code.token,
        name,
        public_key: base64Encode(key.publicKey),
      }),
    );
    const { deviceId, scopes } = enrollResult(enrolled);

    // efrd keeps the device now, so a cancel no longer stops the run: a record that the
    // phone drops would leave a device at efrd that no phone can use.
    this.step({ step: "saving" });
    const records = await this.deps.devices.loadAll().catch(() => []);
    const previous = records.find((record) => record.daemonId === this.code.daemonId);
    const record: PairedDevice = {
      deviceId,
      daemonId: this.code.daemonId,
      host: this.code.host,
      port: this.code.port,
      name,
      keyAlias: alias,
      scopes,
      pairedAt: this.deps.now().toISOString(),
    };
    const { fqdn } = this.code;
    const device: PairedDevice = fqdn === undefined ? record : { ...record, fqdn };
    await this.deps.devices.upsert(device).catch((error: unknown) => {
      throw new Failure({
        kind: "failed",
        message: `the phone could not keep the pairing: ${messageOf(error)}`,
      });
    });
    // The new record names the new key; from here on a failure must not remove it.
    this.newAlias = undefined;
    if (previous !== undefined && previous.keyAlias !== alias) {
      await this.deps.keys.remove(previous.keyAlias).catch(() => {});
    }
    return device;
  }

  /** Removes what a failed run made: the new key. */
  async cleanUp(): Promise<void> {
    const alias = this.newAlias;
    this.newAlias = undefined;
    if (alias === undefined) {
      return;
    }
    const remove = (): Promise<void> => this.deps.keys.remove(alias).catch(() => {});
    // After a cancel the key store may still make the key; remove it once it exists.
    this.creating?.then(remove, () => {});
    await remove();
  }

  stop(): void {
    this.stopped = true;
    this.removeCancel();
    this.connection?.close();
  }

  /**
   * Opens a connection without a device: the socket and `hello`, which fails when the
   * daemon or the protocol is not the one of the code, before the token goes out.
   */
  private connect(): Promise<Connection> {
    const url = daemonUrl(this.code);
    this.step({ step: "connecting", url });
    const opening = Connection.open({
      url,
      sockets: this.deps.sockets,
      clock: this.deps.clock,
      client: this.deps.client,
      daemonId: this.code.daemonId,
    });
    void this.closeIfLate(opening);
    return this.until(opening).catch((error: unknown) => {
      throw error instanceof ConnectionError ? new Failure(openFailure(error)) : error;
    });
  }

  /** A connection that opens after a cancel, or after the run ended, must still close. */
  private async closeIfLate(opening: Promise<Connection>): Promise<void> {
    let connection: Connection;
    try {
      connection = await opening;
    } catch {
      // NOTE: `connect` sees the failure.
      return;
    }
    this.connection = connection;
    if (this.stopped || this.options.signal?.cancelled === true) {
      connection.close();
    }
  }

  private step(step: PairingStep): void {
    this.options.onStep?.(step);
  }

  /** `promise`, or a `Cancelled` error as soon as the signal is set. */
  private until<T>(promise: Promise<T>): Promise<T> {
    if (this.options.signal?.cancelled === true) {
      promise.catch(() => {});
      return Promise.reject(new Cancelled());
    }
    return Promise.race([promise, this.cancelled]);
  }
}

/** A failure that the flow knows how to name. */
class Failure extends Error {
  constructor(readonly failure: PairingFailure) {
    super(failure.kind);
  }
}

/** A JSON object. */
type JsonObject = Readonly<Record<string, unknown>>;

function objectOf(value: unknown): JsonObject | undefined {
  return isJsonObject(value) ? value : undefined;
}

/** A promise that rejects when `reject` is called, and that is never unhandled. */
function rejection(): { readonly promise: Promise<never>; reject(error: Error): void } {
  let reject: ((error: Error) => void) | undefined;
  const promise = new Promise<never>((_, fail) => {
    reject = fail;
  });
  // NOTE: the promise is only raced; a cancel before the race must not be unhandled.
  promise.catch(() => {});
  return { promise, reject: (error) => reject?.(error) };
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function enrollResult(value: unknown): { deviceId: string; scopes: string[] } {
  const result = objectOf(value);
  const deviceId = result?.["device_id"];
  const scopes = stringArray(result?.["scopes"]);
  if (typeof deviceId !== "string" || !UUID.test(deviceId) || scopes === undefined) {
    throw new Failure({ kind: "failed", message: "the enroll result has no device_id or scopes" });
  }
  return { deviceId, scopes };
}

function failureOf(error: unknown, signal: CancelSignal | undefined): PairingFailure {
  if (error instanceof Failure) {
    return error.failure;
  }
  // A cancel closes the connection, so the request that waits fails as closed.
  if (error instanceof Cancelled || signal?.cancelled === true) {
    return { kind: "cancelled" };
  }
  if (error instanceof ServerError) {
    switch (error.body.code) {
      case "unauthorized":
        return { kind: "token_refused", message: error.message };
      case "forbidden":
        return { kind: "refused", message: error.message };
      default:
        return { kind: "failed", message: error.message };
    }
  }
  if (error instanceof ConnectionError) {
    return { kind: "failed", message: error.message };
  }
  if (error instanceof DeviceKeyError) {
    return { kind: "key_failed", message: error.message };
  }
  return { kind: "failed", message: messageOf(error) };
}

/** Why a connection did not open: no socket or no hello, or the wrong daemon. */
function openFailure(error: ConnectionError): PairingFailure {
  switch (error.kind) {
    case "socket":
    case "timeout":
      return { kind: "unreachable" };
    case "daemon_changed":
      return { kind: "wrong_daemon" };
    case "protocol_mismatch": {
      const failure: { kind: "protocol_mismatch"; protocol?: number; version?: string } = {
        kind: "protocol_mismatch",
      };
      if (error.details.daemonProtocol !== undefined) {
        failure.protocol = error.details.daemonProtocol;
      }
      if (error.details.daemonVersion !== undefined) {
        failure.version = error.details.daemonVersion;
      }
      return failure;
    }
    case "closed":
      // NOTE: only the flow closes a connection that opens: a cancel.
      return { kind: "cancelled" };
    default:
      return { kind: "failed", message: error.message };
  }
}

/** Any failure of the key store, the native module's own errors too, as a key failure. */
function keyError(error: unknown): never {
  throw error instanceof DeviceKeyError ? error : new DeviceKeyError(messageOf(error));
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
