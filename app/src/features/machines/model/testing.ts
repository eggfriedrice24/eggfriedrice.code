// Fakes for the tests of the supervisor and the runtime: a stream factory that records
// what the supervisor does with each stream, and paired machines with their keys. Only
// test files import this module.

import { createAtom } from "@tanstack/store";

import type { SubscriptionStatus } from "@/lib/net/client";
import type { SubscribeItem } from "@/lib/protocol/wire";
import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import { deviceKeyStore, type DeviceKeyStore } from "@/features/pairing/model/device-key";
import type { PairedDevice } from "@/features/pairing/model/paired-device";
import { FakeKeyBinding, MemoryPairedDevices } from "@/features/pairing/model/testing";
import type { ConversationRef } from "./machine-ref";
import type { ConversationStream, ConversationStreamFactory, StreamDeps } from "./stream-factory";

/** A stream that records its items, its states and its life. */
export class FakeStream implements ConversationStream {
  readonly store = createAtom<StreamState>(EMPTY_STREAM);
  readonly items: SubscribeItem[] = [];
  readonly statuses: SubscriptionStatus["state"][] = [];
  hydrated = false;
  disposed = false;
  /** The cursor that the cache gave, or the last event's sequence number. */
  seq: number | undefined;
  private finish: (() => void) | undefined;
  private readonly gate: Promise<void>;

  constructor(
    readonly ref: ConversationRef,
    readonly deps: StreamDeps,
    options: { readonly cachedSeq?: number; readonly slowCache?: boolean } = {},
  ) {
    this.seq = options.cachedSeq;
    this.gate =
      options.slowCache === true
        ? new Promise<void>((resolve) => {
            this.finish = resolve;
          })
        : Promise.resolve();
  }

  cursor(): number | undefined {
    return this.seq;
  }

  item(item: SubscribeItem): void {
    this.items.push(item);
    if (item.kind === "event") {
      this.seq = item.seq;
    }
  }

  status(status: SubscriptionStatus): void {
    this.statuses.push(status.state);
  }

  async hydrate(): Promise<void> {
    await this.gate;
    this.hydrated = true;
  }

  /** Lets a slow cache finish its read. */
  finishCache(): void {
    this.finish?.();
  }

  dispose(): void {
    this.disposed = true;
  }
}

/** A stream factory that keeps every stream it made. */
export class FakeStreams {
  readonly made: FakeStream[] = [];
  /** The options of the next streams. */
  options: { readonly cachedSeq?: number; readonly slowCache?: boolean } = {};

  readonly factory: ConversationStreamFactory = (ref, deps) => {
    const stream = new FakeStream(ref, deps, this.options);
    this.made.push(stream);
    return stream;
  };

  /** The newest stream. */
  get last(): FakeStream {
    const stream = this.made.at(-1);
    if (stream === undefined) {
      throw new Error("no stream was made");
    }
    return stream;
  }
}

/** The daemon ids of the test machines. */
export const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
export const LAPTOP = "019a9b1c-3d00-7a10-8b20-0000000000d2";

const MACHINES: Readonly<Record<string, { name: string; host: string; deviceId: string }>> = {
  [DESKTOP]: {
    name: "desktop",
    host: "100.64.0.1",
    deviceId: "019a9b1c-3d00-7a10-8b20-0000000000e1",
  },
  [LAPTOP]: {
    name: "laptop",
    host: "100.64.0.2",
    deviceId: "019a9b1c-3d00-7a10-8b20-0000000000e2",
  },
};

/** The pairing record of test machine `daemonId` (`DESKTOP` or `LAPTOP`). */
export function machineRecord(
  daemonId: string,
  overrides: Partial<PairedDevice> = {},
): PairedDevice {
  const machine = MACHINES[daemonId];
  if (machine === undefined) {
    throw new Error(`no test machine ${daemonId}`);
  }
  return {
    deviceId: machine.deviceId,
    daemonId,
    host: machine.host,
    port: 7419,
    fqdn: `${machine.name}.tail3f2a.ts.net`,
    name: "iPhone 17 Pro",
    keyAlias: `device-${machine.name}`,
    scopes: ["read", "operate", "approve"],
    pairedAt: "2026-10-26T09:00:00.000Z",
    ...overrides,
  };
}

/** Key stores and record stores with the test machines paired. */
export async function pairedStores(records: readonly PairedDevice[]): Promise<{
  readonly binding: FakeKeyBinding;
  readonly keys: DeviceKeyStore;
  readonly devices: MemoryPairedDevices;
}> {
  const binding = new FakeKeyBinding();
  const keys = deviceKeyStore(binding);
  for (const record of records) {
    await keys.create(record.keyAlias);
  }
  return { binding, keys, devices: new MemoryPairedDevices(records) };
}
