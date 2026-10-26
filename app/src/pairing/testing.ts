// Fakes for the tests of pairing. Only test files import this module.

import { encodeBase64 } from "./base64";
import type { OpenSocket, Schedule, SocketHandlers } from "./connection";
import type { DeviceKeyBinding } from "./device_key";
import { isJsonObject } from "./json";
import type { PairedDevice, PairedDeviceStore } from "./paired_device";

/** One request frame that the client sent. */
export interface SentRequest {
  readonly id: number;
  readonly method: string;
  readonly params: unknown;
}

/** Parses a request frame; fails the test when the client sent something else. */
export function parseRequest(text: string): SentRequest {
  const value: unknown = JSON.parse(text);
  if (
    !isJsonObject(value) ||
    typeof value["id"] !== "number" ||
    typeof value["method"] !== "string"
  ) {
    throw new Error(`not a request frame: ${text}`);
  }
  return { id: value["id"], method: value["method"], params: value["params"] };
}

/** Runs `run` after the promise jobs that are queued now. */
export async function later(run: () => void): Promise<void> {
  await Promise.resolve();
  run();
}

/** A timer queue that runs only when a test moves its time. */
export class FakeTimers {
  private now = 0;
  private timers: { at: number; run: () => void; live: boolean }[] = [];

  readonly schedule: Schedule = (ms, run) => {
    const timer = { at: this.now + ms, run, live: true };
    this.timers.push(timer);
    return () => {
      timer.live = false;
    };
  };

  /** Moves time by `ms` and runs each timer that is due, in order. */
  advance(ms: number): void {
    this.now += ms;
    const due = this.timers.filter((timer) => timer.live && timer.at <= this.now);
    this.timers = this.timers.filter((timer) => !due.includes(timer));
    due.sort((a, b) => a.at - b.at);
    for (const timer of due) {
      timer.live = false;
      timer.run();
    }
  }

  /** The number of timers that may still run. */
  get pending(): number {
    return this.timers.filter((timer) => timer.live).length;
  }
}

/** One fake WebSocket: the test sees what the client sends and answers by hand. */
export class FakeSocket {
  readonly sent: string[] = [];
  closedByClient = false;

  constructor(
    readonly url: string,
    readonly handlers: SocketHandlers,
  ) {}

  /** The requests that the client sent, parsed. */
  get requests(): SentRequest[] {
    return this.sent.map(parseRequest);
  }

  open(): void {
    this.handlers.open();
  }

  /** Sends one server frame as JSON text. */
  frame(frame: unknown): void {
    this.handlers.message(JSON.stringify(frame));
  }

  /** The result of request `id`: an item, then the end. */
  result(id: number, item: unknown): void {
    this.frame({ id, item });
    this.frame({ id, end: true });
  }

  error(id: number | null, code: string, message: string, data?: unknown): void {
    this.frame(
      id === null
        ? { error: { code, message, ...(data === undefined ? {} : { data }) } }
        : { id, error: { code, message, ...(data === undefined ? {} : { data }) } },
    );
  }

  close(code = 1000, reason = ""): void {
    this.handlers.close(code, reason);
  }
}

/** An `OpenSocket` that records each socket it opens. */
export function fakeSockets(): { readonly open: OpenSocket; readonly sockets: FakeSocket[] } {
  const sockets: FakeSocket[] = [];
  const open: OpenSocket = (url, handlers) => {
    const socket = new FakeSocket(url, handlers);
    sockets.push(socket);
    return {
      send: (text) => socket.sent.push(text),
      close: () => {
        socket.closedByClient = true;
      },
    };
  };
  return { open, sockets };
}

/** Waits until every promise job that is queued now has run. */
export async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) {
    // oxlint-disable-next-line eslint/no-await-in-loop
    await Promise.resolve();
  }
}

/** The 65-byte public key of the fixture `device_enroll_params.json`: 0x04, then 1 to 64. */
export const FIXTURE_PUBLIC_KEY = Uint8Array.from({ length: 65 }, (_, i) => (i === 0 ? 4 : i));

/** A DER signature of the right shape (r and s of 32 bytes each, without a sign byte). */
export const FAKE_SIGNATURE = Uint8Array.from([
  0x30,
  0x44,
  0x02,
  0x20,
  ...Array.from({ length: 32 }, () => 0x11),
  0x02,
  0x20,
  ...Array.from({ length: 32 }, () => 0x22),
]);

/** A native key module in memory. Each key is the fixture key with its last byte set. */
export class FakeKeyBinding implements DeviceKeyBinding {
  readonly keys = new Map<string, Uint8Array>();
  readonly signed: { alias: string; message: string }[] = [];
  failCreate: Error | undefined;
  hardwareBacked = true;
  private made = 0;

  isHardwareBacked(): boolean {
    return this.hardwareBacked;
  }

  createKey(alias: string): Promise<string> {
    if (this.failCreate !== undefined) {
      return Promise.reject(this.failCreate);
    }
    const key = FIXTURE_PUBLIC_KEY.slice();
    key[64] = ++this.made;
    this.keys.set(alias, key);
    return Promise.resolve(encodeBase64(key));
  }

  publicKey(alias: string): Promise<string | null> {
    const key = this.keys.get(alias);
    return Promise.resolve(key === undefined ? null : encodeBase64(key));
  }

  sign(alias: string, message: string): Promise<string> {
    if (!this.keys.has(alias)) {
      return Promise.reject(new Error(`There is no device key '${alias}'`));
    }
    this.signed.push({ alias, message });
    return Promise.resolve(encodeBase64(FAKE_SIGNATURE));
  }

  deleteKey(alias: string): Promise<void> {
    this.keys.delete(alias);
    return Promise.resolve();
  }
}

/** A pairing record store in memory. */
export class MemoryPairedDevices implements PairedDeviceStore {
  device: PairedDevice | null = null;
  failSave: Error | undefined;

  load(): Promise<PairedDevice | null> {
    return Promise.resolve(this.device);
  }

  save(device: PairedDevice): Promise<void> {
    if (this.failSave !== undefined) {
      return Promise.reject(this.failSave);
    }
    this.device = device;
    return Promise.resolve();
  }

  remove(): Promise<void> {
    this.device = null;
    return Promise.resolve();
  }
}
