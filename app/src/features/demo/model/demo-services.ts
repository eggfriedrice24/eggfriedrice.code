// The demo machines in two forms (plan section 4, W4-DEMO-MEASURE):
//
// - `withDemoMachines` gives the pairing effects of the app a socket factory that sends
//   the addresses of the demo machines to the demo server. The runtime of the app stays
//   the same; the gallery pairs the demo machines through the real pairing flow.
// - `sandboxServices` makes services of the app of their own, with stores in memory and
//   only demo machines, for a board that needs a setup that the app's own runtime cannot
//   have, such as exactly one machine. Nothing of it reaches the disk, the secure store
//   or the network, and it touches no global listener of the app.
//
// Pure: every effect comes in as a parameter.

import type { AsyncStorage } from "@tanstack/react-query-persist-client";

import type { Clock } from "@/lib/net/clock";
import type { SocketFactory } from "@/lib/net/socket";
import { base64Encode } from "@/lib/protocol/bytes";
import type { QueryManagers } from "@/lib/query/managers";
import { MemoryEventCaches } from "@/lib/store/event-cache";
import type { FrameScheduler } from "@/lib/store/frame-store";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { SqlDatabase } from "@/lib/store/sqlite-cache";
import { MemoryOutboxStorage } from "@/features/conversation/model/outbox-storage";
import { createAppServices, type AppServices } from "@/features/navigation/model/app-services";
import {
  deviceKeyStore,
  type DeviceKeyBinding,
  type DeviceKeyStore,
} from "@/features/pairing/model/device-key";
import type { PairingDeps } from "@/features/pairing/model/pair";
import {
  withDevice,
  type PairedDevice,
  type PairedDeviceStore,
} from "@/features/pairing/model/paired-device";
import { DEMO_MACHINES, type DemoMachineKey } from "./demo-machines";
import type { DemoServer } from "./demo-server";

/** The name that the phone gives itself when it pairs with a demo machine. */
export const DEMO_DEVICE_NAME = "iPhone (demo)";

/** The pairing effects of the app, with the demo machines on the demo server. */
export function withDemoMachines(pairing: PairingDeps, server: DemoServer): PairingDeps {
  return { ...pairing, sockets: server.sockets(pairing.sockets) };
}

/** A key-value store in memory. */
export class MemoryKeyValue implements KeyValueStorage, AsyncStorage {
  private readonly values = new Map<string, string>();

  getItemSync(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  getItem(key: string): Promise<string | null> {
    return Promise.resolve(this.getItemSync(key));
  }

  setItem(key: string, value: string): Promise<void> {
    this.values.set(key, value);
    return Promise.resolve();
  }

  removeItem(key: string): Promise<void> {
    this.values.delete(key);
    return Promise.resolve();
  }
}

/** Pairing records in memory. */
export class MemoryPairings implements PairedDeviceStore {
  private records: readonly PairedDevice[] = [];

  loadAll(): Promise<readonly PairedDevice[]> {
    return Promise.resolve(this.records);
  }

  upsert(device: PairedDevice): Promise<void> {
    this.records = withDevice(this.records, device);
    return Promise.resolve();
  }

  remove(daemonId: string): Promise<void> {
    this.records = this.records.filter((record) => record.daemonId !== daemonId);
    return Promise.resolve();
  }
}

/** A DER signature of the right shape. The demo server checks no signature. */
const SIGNATURE = base64Encode(
  Uint8Array.from([
    0x30,
    0x44,
    0x02,
    0x20,
    ...Array.from({ length: 32 }, () => 0x11),
    0x02,
    0x20,
    ...Array.from({ length: 32 }, () => 0x22),
  ]),
);

/** A key module in memory: each key is an uncompressed P-256 point of the right size. */
export class MemoryKeyBinding implements DeviceKeyBinding {
  private readonly keys = new Map<string, string>();

  isHardwareBacked(): boolean {
    return false;
  }

  createKey(alias: string): Promise<string> {
    const key = Uint8Array.from({ length: 65 }, (_, i) =>
      i === 0 ? 4 : (i * 7 + alias.length) % 256,
    );
    const text = base64Encode(key);
    this.keys.set(alias, text);
    return Promise.resolve(text);
  }

  publicKey(alias: string): Promise<string | null> {
    return Promise.resolve(this.keys.get(alias) ?? null);
  }

  sign(): Promise<string> {
    return Promise.resolve(SIGNATURE);
  }

  deleteKey(alias: string): Promise<void> {
    this.keys.delete(alias);
    return Promise.resolve();
  }
}

/** A database that the sandbox never opens: its caches and its outbox are in memory. */
const NO_DATABASE: SqlDatabase = {
  execAsync: () => Promise.reject(new Error("the sandbox has no database")),
  runAsync: () => Promise.reject(new Error("the sandbox has no database")),
  getAllAsync: () => Promise.reject(new Error("the sandbox has no database")),
};

/** Managers of Query that hear nothing, so the sandbox leaves the app's managers alone. */
const QUIET_MANAGERS: QueryManagers = {
  focus: { setEventListener: () => undefined },
  online: { setEventListener: () => undefined },
};

/** A socket factory for any address that is not a demo machine: it never connects. */
const NO_NETWORK: SocketFactory = () => {
  throw new Error("the sandbox reaches no network");
};

/** What a sandbox needs. */
export interface SandboxOptions {
  readonly server: DemoServer;
  /** The demo machines that the sandbox has paired, in order. */
  readonly machines: readonly DemoMachineKey[];
  readonly schedule: FrameScheduler;
  /** A random value in [0, 1), for the backoff ladders. */
  readonly random?: () => number;
  readonly onError?: (error: unknown) => void;
}

/** The alias of the device key of demo machine `key` in a sandbox. */
function aliasOf(key: DemoMachineKey): string {
  return `demo-${key}`;
}

/** A sandbox and its stores. */
export interface Sandbox {
  readonly services: AppServices;
  readonly keys: DeviceKeyStore;
  readonly devices: MemoryPairings;
}

/** Services of the app with only the demo machines `machines`, all in memory. */
export async function sandboxServices(options: SandboxOptions): Promise<Sandbox> {
  const { server } = options;
  const clock: Clock = server.clock;
  const keys = deviceKeyStore(new MemoryKeyBinding());
  const devices = new MemoryPairings();
  const machines = DEMO_MACHINES.filter((machine) => options.machines.includes(machine.key));
  await Promise.all(machines.map((machine) => keys.create(aliasOf(machine.key))));
  // The records keep the order of `machines`: the store adds each one at the end.
  await Promise.all(
    machines.map((machine) =>
      devices.upsert({
        deviceId: server.newId(),
        daemonId: machine.daemonId,
        host: machine.host,
        port: machine.port,
        fqdn: machine.fqdn,
        name: DEMO_DEVICE_NAME,
        keyAlias: aliasOf(machine.key),
        scopes: ["read", "operate", "approve"],
        pairedAt: new Date(clock.now()).toISOString(),
      }),
    ),
  );
  const keyValue = new MemoryKeyValue();
  let aliases = 0;
  const services = createAppServices({
    pairing: {
      sockets: server.sockets(NO_NETWORK),
      clock,
      keys,
      devices,
      sha256: () => Promise.resolve(new Uint8Array(32)),
      newKeyAlias: () => {
        aliases += 1;
        return `demo-new-${aliases}`;
      },
      now: () => new Date(clock.now()),
      client: "efr-app demo",
    },
    random: options.random ?? Math.random,
    schedule: options.schedule,
    clock,
    now: () => clock.now(),
    newId: () => server.newId(),
    openDatabase: () => Promise.resolve(NO_DATABASE),
    openEventCaches: () => Promise.resolve(new MemoryEventCaches()),
    openOutboxStorage: () => Promise.resolve(new MemoryOutboxStorage()),
    keyValue,
    queryStorage: new MemoryKeyValue(),
    appVersion: "0.0.0-demo",
    lifecycle: { followAppState: () => () => undefined, followNetwork: () => () => undefined },
    followOnline: () => () => undefined,
    setHapticsEnabled: () => undefined,
    onError: options.onError ?? (() => undefined),
    queryManagers: QUIET_MANAGERS,
  });
  await services.loaded;
  return { services, keys, devices };
}
