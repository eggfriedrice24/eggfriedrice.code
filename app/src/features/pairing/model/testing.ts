// Fakes for the tests of pairing. Only test files import this module.

import { QueryClient } from "@tanstack/react-query";
import { createAtom, type Atom } from "@tanstack/store";

import type { PairVariables } from "@/features/machines/machine-queries";
import { machineOf, type Machine } from "@/features/machines/model/machine";
import type { MachineId } from "@/features/machines/model/machine-ref";
import { loadedRegistry, type RegistryState } from "@/features/machines/model/machine-registry";
import { base64Encode } from "@/lib/protocol/bytes";
import { isJsonObject } from "@/lib/protocol/envelope";
import type { DeviceKeyBinding } from "./device-key";
import { PairingError, type PairingFailure } from "./pair";
import { withDevice } from "./paired-device";
import type { PairedDevice, PairedDeviceStore } from "./paired-device";
import {
  createPairingSession,
  type PairingHaptic,
  type PairingSession,
  type PairingSessionDeps,
} from "./pairing-session";
import type { ScannerApi } from "./scanner";

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

/** Waits until every promise job that is queued now has run. */
export async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) {
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
    return Promise.resolve(base64Encode(key));
  }

  publicKey(alias: string): Promise<string | null> {
    const key = this.keys.get(alias);
    return Promise.resolve(key === undefined ? null : base64Encode(key));
  }

  sign(alias: string, message: string): Promise<string> {
    if (!this.keys.has(alias)) {
      return Promise.reject(new Error(`There is no device key '${alias}'`));
    }
    this.signed.push({ alias, message });
    return Promise.resolve(base64Encode(FAKE_SIGNATURE));
  }

  deleteKey(alias: string): Promise<void> {
    this.keys.delete(alias);
    return Promise.resolve();
  }
}

/** A pairing record store in memory, for many machines. */
export class MemoryPairedDevices implements PairedDeviceStore {
  /** The records, in the order of the first pairing with each machine. */
  devices: readonly PairedDevice[] = [];
  failSave: Error | undefined;
  failLoad: Error | undefined;

  constructor(devices: readonly PairedDevice[] = []) {
    this.devices = devices;
  }

  /** The record of daemon `daemonId`, or undefined. */
  of(daemonId: string): PairedDevice | undefined {
    return this.devices.find((device) => device.daemonId === daemonId);
  }

  loadAll(): Promise<readonly PairedDevice[]> {
    if (this.failLoad !== undefined) {
      return Promise.reject(this.failLoad);
    }
    return Promise.resolve(this.devices);
  }

  upsert(device: PairedDevice): Promise<void> {
    if (this.failSave !== undefined) {
      return Promise.reject(this.failSave);
    }
    this.devices = withDevice(this.devices, device);
    return Promise.resolve();
  }

  remove(daemonId: string): Promise<void> {
    this.devices = this.devices.filter((device) => device.daemonId !== daemonId);
    return Promise.resolve();
  }
}

/** The system scanner in memory: the test shows it codes by hand. */
export class FakeScanner implements ScannerApi {
  available = true;
  granted = true;
  launched = 0;
  dismissed = 0;
  failLaunch = false;
  private listeners = new Set<(data: string) => void>();

  requestPermission(): Promise<boolean> {
    return Promise.resolve(this.granted);
  }

  launch(): Promise<void> {
    this.launched++;
    return this.failLaunch ? Promise.reject(new Error("no scanner")) : Promise.resolve();
  }

  dismiss(): Promise<void> {
    this.dismissed++;
    return Promise.resolve();
  }

  onScanned(listener: (data: string) => void): { remove(): void } {
    this.listeners.add(listener);
    return { remove: () => this.listeners.delete(listener) };
  }

  get listening(): number {
    return this.listeners.size;
  }

  show(data: string): void {
    for (const listener of this.listeners) {
      listener(data);
    }
  }
}

/** The daemon of `SESSION_CODE`. */
export const SESSION_DAEMON = "019a9b1c-3d00-7a10-8b20-000000000007";

/** A pairing code of machine `desktop` (desktop.tail3f2a.ts.net). */
export const SESSION_CODE = `efr://pair?v=1&d=${SESSION_DAEMON}&h=100.101.102.103&p=7878&f=desktop.tail3f2a.ts.net&t=q83vASNFZ4mrze8BI0VniQ`;

/** The record of a machine that a test pairs. */
export function pairedRecord(daemonId: MachineId, name: string): PairedDevice {
  return {
    deviceId: "019a9b1c-3d00-7a10-8b20-000000000006",
    daemonId,
    host: "100.101.102.103",
    port: 7878,
    fqdn: `${name}.tail3f2a.ts.net`,
    name: "iPhone 17 Pro",
    keyAlias: `device-${name}`,
    scopes: ["read", "operate", "approve"],
    pairedAt: "2026-10-26T09:00:00.000Z",
  };
}

/** The machine of `pairedRecord(daemonId, name)`. */
export function pairedMachine(daemonId: MachineId, name: string): Machine {
  return machineOf(pairedRecord(daemonId, name), false);
}

/** One pair mutation that a test answers by hand. */
export interface PendingPair {
  readonly variables: PairVariables;
  /** Ends the pairing with machine `machine`. */
  succeed(machine: Machine): void;
  /** Ends the pairing with `failure`. */
  fail(failure: PairingFailure): void;
}

/** A pairing session with fakes for every effect, and what the session did with them. */
export interface FakePairingSession {
  readonly session: PairingSession;
  readonly scanner: FakeScanner;
  readonly registry: Atom<RegistryState>;
  readonly client: QueryClient;
  /** The pair mutations, oldest first. */
  readonly pairs: PendingPair[];
  readonly haptics: PairingHaptic[];
  readonly announced: string[];
  readonly selected: MachineId[];
  readonly opened: ("tailscale" | "settings")[];
  /** The AppState listeners that are on now. */
  readonly appState: Set<(active: boolean) => void>;
}

/** The options of `fakePairingSession`. */
export interface FakePairingOptions {
  /** False for a build without a system scanner. */
  readonly scanner?: boolean;
  /** The paired machines. */
  readonly machines?: readonly Machine[];
}

/** A pairing session with fakes. */
export function fakePairingSession(options: FakePairingOptions = {}): FakePairingSession {
  const scanner = new FakeScanner();
  const registry = createAtom<RegistryState>(loadedRegistry(options.machines ?? []));
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const pairs: PendingPair[] = [];
  const haptics: PairingHaptic[] = [];
  const announced: string[] = [];
  const selected: MachineId[] = [];
  const opened: ("tailscale" | "settings")[] = [];
  const appState = new Set<(active: boolean) => void>();
  const deps: PairingSessionDeps = {
    client,
    mutation: {
      mutationKey: ["machine", "pair"],
      mutationFn: (variables: PairVariables) =>
        new Promise<Machine>((resolve, reject) => {
          pairs.push({
            variables,
            succeed: resolve,
            fail: (failure) => reject(new PairingError(failure)),
          });
        }),
    },
    registry,
    scanner: options.scanner === false ? null : scanner,
    deviceName: () => "iPhone 17 Pro",
    haptic: (kind) => haptics.push(kind),
    announce: (text) => announced.push(text),
    followAppState: (listener) => {
      appState.add(listener);
      return () => appState.delete(listener);
    },
    openTailscale: () => opened.push("tailscale"),
    openSettings: () => opened.push("settings"),
    selectMachine: (machine) => selected.push(machine),
  };
  return {
    session: createPairingSession(deps),
    scanner,
    registry,
    client,
    pairs,
    haptics,
    announced,
    selected,
    opened,
    appState,
  };
}
