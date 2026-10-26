// Fakes for the tests of the connection, the client and the store: a clock that moves
// only when a test moves it, sockets that a test drives frame by frame, and a device
// key. Only test files import this module.

import { isJsonObject } from "../core/envelope";
import type { HelloResult } from "../protocol/generated";
import type { Clock, Timer } from "./clock";
import type { DeviceKey } from "./device_key";
import { CLOSE_ABNORMAL, type Socket, type SocketFactory, type SocketListener } from "./socket";

interface FakeTimer {
  readonly at: number;
  readonly order: number;
  readonly run: () => void;
  cancelled: boolean;
}

/** A clock whose time moves only in `advance`. Due timers run in time order. */
export class FakeClock implements Clock {
  private time: number;
  private order = 0;
  private timers: FakeTimer[] = [];

  constructor(start = 1_000_000) {
    this.time = start;
  }

  now(): number {
    return this.time;
  }

  setTimer(delayMs: number, run: () => void): Timer {
    const timer: FakeTimer = {
      at: this.time + Math.max(0, delayMs),
      order: this.order++,
      run,
      cancelled: false,
    };
    this.timers.push(timer);
    return {
      cancel: () => {
        timer.cancelled = true;
      },
    };
  }

  /** The number of timers that have not run and are not cancelled. */
  get pendingTimers(): number {
    return this.timers.filter((timer) => !timer.cancelled).length;
  }

  /** Moves the time forward by `ms`, and runs each timer that falls due on the way. */
  async advance(ms: number): Promise<void> {
    const end = this.time + ms;
    for (;;) {
      await flush();
      const due = this.timers
        .filter((timer) => !timer.cancelled && timer.at <= end)
        .sort((a, b) => a.at - b.at || a.order - b.order)[0];
      if (due === undefined) {
        break;
      }
      this.time = Math.max(this.time, due.at);
      due.cancelled = true;
      due.run();
    }
    this.timers = this.timers.filter((timer) => !timer.cancelled);
    this.time = end;
    await flush();
  }
}

/** Lets every pending promise callback run. */
export async function flush(): Promise<void> {
  for (let i = 0; i < 20; i++) {
    await Promise.resolve();
  }
}

/** One parsed frame that the app sent. */
export type SentFrame = Readonly<Record<string, unknown>>;

/** A socket that a test drives. */
export class FakeSocket implements Socket {
  readonly url: string;
  readonly sent: SentFrame[] = [];
  closedWith: number | undefined;
  private readonly listener: SocketListener;
  private dead = false;

  constructor(url: string, listener: SocketListener) {
    this.url = url;
    this.listener = listener;
  }

  send(text: string): void {
    if (this.dead || this.closedWith !== undefined) {
      throw new Error("the socket is not open");
    }
    const frame: unknown = JSON.parse(text);
    if (!isJsonObject(frame)) {
      throw new Error("the app sent a frame that is not an object");
    }
    this.sent.push(frame);
  }

  close(code?: number): void {
    this.closedWith ??= code ?? 1000;
  }

  /** The socket opens. */
  open(): void {
    this.listener.opened();
  }

  /** efrd sends one frame. */
  receive(frame: unknown): void {
    this.listener.message(JSON.stringify(frame));
  }

  /** efrd sends one raw message. */
  receiveRaw(data: unknown): void {
    this.listener.message(data);
  }

  /** The socket fails or efrd closes it. */
  drop(code = CLOSE_ABNORMAL): void {
    this.dead = true;
    this.listener.closed(code, "");
  }

  /** The requests that the app sent with method `method`, oldest first. */
  requests(method: string): SentFrame[] {
    return this.sent.filter((frame) => frame["method"] === method);
  }

  /** The id of the last request with method `method`. */
  lastId(method: string): number {
    const id = this.requests(method).at(-1)?.["id"];
    if (typeof id !== "number") {
      throw new Error(`the app sent no ${method} request`);
    }
    return id;
  }

  /** Answers the last request with method `method` with `result`, as item and end. */
  answer(method: string, result: unknown): void {
    const id = this.lastId(method);
    this.receive({ id, item: result });
    this.receive({ id, end: true });
  }

  /** Answers the last request with method `method` with an error. */
  refuse(method: string, code: string, message = "refused", data?: unknown): void {
    const id = this.lastId(method);
    this.receive({ id, error: data === undefined ? { code, message } : { code, message, data } });
  }
}

/** A socket factory that keeps every socket it made. */
export class FakeSockets {
  readonly made: FakeSocket[] = [];
  readonly factory: SocketFactory = (url, listener) => {
    const socket = new FakeSocket(url, listener);
    this.made.push(socket);
    return socket;
  };

  /** The newest socket. */
  get last(): FakeSocket {
    const socket = this.made.at(-1);
    if (socket === undefined) {
      throw new Error("no socket was made");
    }
    return socket;
  }
}

export const DAEMON_ID = "019a9b1c-3d00-7a10-8b20-000000000007";
export const DEVICE_ID = "019a9b1c-3d00-7a10-8b20-000000000006";

/** A hello result of the tailnet listener, as efrd sends it. */
export function remoteHello(overrides: Partial<HelloResult> = {}): HelloResult {
  return {
    daemon_id: DAEMON_ID,
    protocol: 1,
    version: "0.0.6",
    capabilities: { drafts_coalesced: true, snapshot_live: true, device_prove: true },
    paths: {
      scratch_root: "/home/me/.local/share/efr/scratch",
      data_dir: "/home/me/.local/share/efr",
    },
    challenge: "3q2-7wAAAAAAAAAAAAAAAA",
    prove: { peer_node: "nPHONE1CNTRL", daemon_node: "nDESK1CNTRL", daemon_tailnet: "T1234CNTRL" },
    ...overrides,
  };
}

/** A device key that signs with a fixed answer and records what it signed. */
export class FakeKey implements DeviceKey {
  readonly signed: Uint8Array[] = [];
  failing = false;

  publicKey(): Promise<Uint8Array> {
    return Promise.resolve(Uint8Array.from({ length: 65 }, (_, i) => (i === 0 ? 4 : i)));
  }

  sign(message: Uint8Array): Promise<Uint8Array> {
    if (this.failing) {
      return Promise.reject(new Error("the key is gone"));
    }
    this.signed.push(message);
    return Promise.resolve(Uint8Array.of(0x30, 0x06, 0x02, 0x01, 0x2a, 0x02, 0x01, 0x2b));
  }
}

/** Opens the newest socket and answers hello and the proof as efrd does. */
export async function completeHandshake(
  socket: FakeSocket,
  hello: HelloResult = remoteHello(),
): Promise<void> {
  socket.open();
  await flush();
  socket.answer("hello", hello);
  await flush();
  if (socket.requests("device.prove").length > 0) {
    socket.answer("device.prove", { scopes: ["read", "operate", "approve"] });
    await flush();
  }
}
