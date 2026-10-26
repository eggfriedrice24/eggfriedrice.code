import { describe, expect, it } from "vitest";

import { ConnectionError } from "@/lib/net/errors";
import {
  FakeClock,
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
  type FakeSocket,
} from "@/lib/net/testing";
import { MemoryEventCaches, type EventCaches } from "@/lib/store/event-cache";
import type { MachineCaller } from "@/lib/query/machine-caller";
import { createConversationStream } from "@/features/conversation/model/conversation-stream";
import { PairingError, type PairingDeps } from "@/features/pairing/model/pair";
import {
  PAIRED_DEVICE_V1_KEY,
  PAIRED_DEVICES_KEY,
  secureStorePairedDevices,
  type PairedDevice,
  type SecureStoreApi,
} from "@/features/pairing/model/paired-device";
import type { MachineId } from "./machine-ref";
import type { NetworkPath } from "./network-path";
import { createRuntime, type Lifecycle, type Runtime, type RuntimeDeps } from "./runtime";
import {
  DESKTOP,
  FakeStreams,
  LAPTOP,
  machineRecord,
  pairedStores,
  type FakeStream,
} from "./testing";

const DESKTOP_URL = "ws://100.64.0.1:7419/";
const LAPTOP_URL = "ws://100.64.0.2:7419/";
const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000000001";
const TOKEN = "q83vASNFZ4mrze8BI0VniQ";
const NEW_DEVICE = "019a9b1c-3d00-7a10-8b20-0000000000f1";

/** The pairing code of the desktop. */
const DESKTOP_CODE = `efr://pair?v=1&d=${DESKTOP}&h=100.64.0.1&p=7419&f=desktop.tail3f2a.ts.net&t=${TOKEN}`;

/** AppState and NetInfo, driven by the test. */
class FakeLifecycle implements Lifecycle {
  readonly appState = new Set<(active: boolean) => void>();
  readonly network = new Set<(path: NetworkPath) => void>();

  followAppState(listener: (active: boolean) => void): () => void {
    this.appState.add(listener);
    return () => this.appState.delete(listener);
  }

  followNetwork(listener: (path: NetworkPath) => void): () => void {
    this.network.add(listener);
    return () => this.network.delete(listener);
  }

  setActive(active: boolean): void {
    for (const listener of this.appState) {
      listener(active);
    }
  }

  setPath(path: NetworkPath): void {
    for (const listener of this.network) {
      listener(path);
    }
  }
}

interface SetupOptions {
  readonly records?: readonly PairedDevice[];
  readonly devices?: PairingDeps["devices"];
  readonly openCache?: () => Promise<EventCaches>;
  readonly noKeyModule?: boolean;
}

async function setup(options: SetupOptions = {}) {
  const records = options.records ?? [machineRecord(DESKTOP), machineRecord(LAPTOP)];
  const stores = await pairedStores(records);
  const devices = options.devices ?? stores.devices;
  const clock = new FakeClock();
  const sockets = new FakeSockets();
  const streams = new FakeStreams();
  const lifecycle = new FakeLifecycle();
  const cache = new MemoryEventCaches();
  const purged: [MachineId, EventCaches | undefined][] = [];
  const errors: unknown[] = [];
  let aliases = 0;
  const pairing: PairingDeps = {
    sockets: sockets.factory,
    clock,
    keys: stores.keys,
    devices,
    sha256: () => Promise.resolve(Uint8Array.from({ length: 32 }, (_, i) => i)),
    newKeyAlias: () => `device-new-${++aliases}`,
    now: () => new Date("2026-10-26T12:00:00Z"),
    client: "efr-app test",
  };
  const deps: RuntimeDeps<FakeStream> = {
    pairing: options.noKeyModule === true ? null : pairing,
    random: () => 0,
    streams: streams.factory,
    schedule: (callback) => {
      callback();
      return () => {};
    },
    openCache: options.openCache ?? (() => Promise.resolve(cache)),
    lifecycle,
    purges: [
      (machine, opened) => {
        purged.push([machine, opened]);
      },
    ],
    onError: (error) => errors.push(error),
  };
  const runtime = createRuntime(deps);
  return { ...stores, clock, sockets, streams, lifecycle, cache, purged, errors, runtime };
}

/** The sockets of the machine at `url`, oldest first. */
function socketsOf(sockets: FakeSockets, url: string): FakeSocket[] {
  return sockets.made.filter((socket) => socket.url === url);
}

function lastOf(sockets: FakeSockets, url: string): FakeSocket {
  const socket = socketsOf(sockets, url).at(-1);
  if (socket === undefined) {
    throw new Error(`no socket to ${url}`);
  }
  return socket;
}

/** Loads the runtime and connects every paired machine. */
async function connected(options: SetupOptions = {}) {
  const parts = await setup(options);
  await parts.runtime.load();
  for (const record of options.records ?? [machineRecord(DESKTOP), machineRecord(LAPTOP)]) {
    const url = `ws://${record.host}:${record.port}/`;
    await completeHandshake(
      lastOf(parts.sockets, url),
      remoteHello({ daemon_id: record.daemonId }),
    );
  }
  return parts;
}

/** Answers the pairing on the newest socket, as efrd does once the user says yes. */
async function confirmPairing(sockets: FakeSockets, daemonId: string): Promise<void> {
  const socket = sockets.last;
  socket.open();
  await flush();
  socket.answer("hello", remoteHello({ daemon_id: daemonId }));
  await flush();
  socket.answer("device.enroll", { device_id: NEW_DEVICE, scopes: ["read", "operate"] });
  await flush();
}

describe("load", () => {
  it("lists the machines and connects them all at the same time", async () => {
    const { runtime, sockets } = await setup();
    expect(runtime.registry.get()).toEqual({ loaded: false, machines: [] });
    await runtime.load();
    expect(runtime.registry.get().loaded).toBe(true);
    expect(runtime.registry.get().machines.map((machine) => [machine.name, machine.fqdn])).toEqual([
      ["desktop", "desktop.tail3f2a.ts.net"],
      ["laptop", "laptop.tail3f2a.ts.net"],
    ]);
    // Both sockets are open before either handshake ends.
    expect(sockets.made.map((socket) => socket.url)).toEqual([DESKTOP_URL, LAPTOP_URL]);
    await completeHandshake(lastOf(sockets, LAPTOP_URL), remoteHello({ daemon_id: LAPTOP }));
    expect(runtime.machine(LAPTOP)?.connection.get().view.kind).toBe("online");
    expect(runtime.machine(DESKTOP)?.connection.get().view.kind).toBe("connecting");
    expect(runtime.load()).toBe(runtime.load());
  });

  it("moves the record of version 1 and connects its machine", async () => {
    const items = new Map<string, string>();
    const api: SecureStoreApi = {
      getItemAsync: (key) => Promise.resolve(items.get(key) ?? null),
      setItemAsync: (key, value) => {
        items.set(key, value);
        return Promise.resolve();
      },
      deleteItemAsync: (key) => {
        items.delete(key);
        return Promise.resolve();
      },
    };
    const record = machineRecord(DESKTOP);
    items.set(
      PAIRED_DEVICE_V1_KEY,
      JSON.stringify({
        version: 1,
        device_id: record.deviceId,
        daemon_id: record.daemonId,
        host: record.host,
        port: record.port,
        fqdn: record.fqdn,
        name: record.name,
        key_alias: record.keyAlias,
        scopes: record.scopes,
        paired_at: record.pairedAt,
      }),
    );
    const { runtime, sockets } = await setup({
      records: [record],
      devices: secureStorePairedDevices(api, 1),
    });
    await runtime.load();
    expect(runtime.registry.get().machines).toMatchObject([
      { id: DESKTOP, name: "desktop", keyMissing: false },
    ]);
    expect(items.has(PAIRED_DEVICE_V1_KEY)).toBe(false);
    expect(items.has(PAIRED_DEVICES_KEY)).toBe(true);
    await completeHandshake(lastOf(sockets, DESKTOP_URL), remoteHello({ daemon_id: DESKTOP }));
    expect(runtime.machine(DESKTOP)?.connection.get().view.kind).toBe("online");
  });

  it("lists a machine whose key is gone, as one that needs a re-pair", async () => {
    const parts = await setup();
    await parts.keys.remove(machineRecord(LAPTOP).keyAlias);
    await parts.runtime.load();
    expect(parts.runtime.registry.get().machines.map((machine) => machine.keyMissing)).toEqual([
      false,
      true,
    ]);
    expect(socketsOf(parts.sockets, LAPTOP_URL)).toHaveLength(0);
    expect(parts.runtime.machine(LAPTOP)?.connection.get().view).toEqual({
      kind: "repair_needed",
      reason: "other",
    });
  });

  it("gives the streams of each machine the cache of that machine", async () => {
    const { runtime, cache } = await setup();
    await runtime.load();
    const desktop = runtime.hold({ machine: DESKTOP, id: CONVERSATION }, "keep");
    const laptop = runtime.hold({ machine: LAPTOP, id: CONVERSATION }, "keep");
    await desktop?.stream.deps.cache?.append([{ seq: 1, json: "{}" }]);
    expect(await cache.forMachine(DESKTOP).load(CONVERSATION)).toEqual([{ seq: 1, json: "{}" }]);
    expect(await laptop?.stream.deps.cache?.load()).toEqual([]);
  });

  it("takes the stream of the conversation feature and calls for the queries", async () => {
    const runtime = createRuntime({
      pairing: null,
      random: () => 0,
      streams: createConversationStream,
      schedule: () => () => {},
    });
    // The types are the test: the screens get the full stream from a hold, and the same
    // runtime goes where a plain runtime and the caller of the queries go.
    const loose: Runtime = runtime;
    const caller: MachineCaller = runtime;
    await runtime.load();
    expect(loose.registry.get()).toEqual({ loaded: true, machines: [] });
    expect(runtime.hold({ machine: DESKTOP, id: CONVERSATION }, "keep")?.stream.rowUi).toBe(
      undefined,
    );
    await expect(caller.call(DESKTOP, "conversations.recent", { limit: 20 })).rejects.toMatchObject(
      { kind: "offline" },
    );
  });

  it("goes on without the cache when it does not open", async () => {
    const failure = new Error("the disk is full");
    const { runtime, errors, streams } = await setup({ openCache: () => Promise.reject(failure) });
    await runtime.load();
    expect(errors).toEqual([failure]);
    runtime.hold({ machine: DESKTOP, id: CONVERSATION }, "keep");
    expect(streams.last.deps.cache).toBeUndefined();
  });

  it("has no machine and cannot pair without the device key module", async () => {
    const { runtime, sockets } = await setup({ noKeyModule: true });
    await runtime.load();
    expect(runtime.registry.get()).toEqual({ loaded: true, machines: [] });
    expect(sockets.made).toHaveLength(0);
    const pairing = runtime.pair(DESKTOP_CODE, { name: "iPhone" });
    await expect(pairing).rejects.toBeInstanceOf(PairingError);
    await expect(pairing).rejects.toMatchObject({ failure: { kind: "key_failed" } });
  });
});

describe("calls", () => {
  it("runs a method on the right machine", async () => {
    const { runtime, sockets } = await connected();
    const recent = runtime.call(LAPTOP, "conversations.recent", { limit: 20 });
    await flush();
    expect(lastOf(sockets, DESKTOP_URL).requests("conversations.recent")).toHaveLength(0);
    lastOf(sockets, LAPTOP_URL).answer("conversations.recent", { conversations: [] });
    await expect(recent).resolves.toEqual({ conversations: [] });
  });

  it("rejects as offline for a machine that is not paired", async () => {
    const { runtime } = await connected();
    const unknown = "019a9b1c-3d00-7a10-8b20-0000000000aa";
    await expect(runtime.call(unknown, "conversations.recent", {})).rejects.toBeInstanceOf(
      ConnectionError,
    );
    await expect(runtime.status(unknown)).rejects.toMatchObject({ kind: "offline" });
    expect(runtime.hold({ machine: unknown, id: CONVERSATION }, "keep")).toBeUndefined();
  });

  it("gives the status of each connected machine", async () => {
    const { runtime, sockets } = await setup();
    await runtime.load();
    await expect(runtime.status(DESKTOP)).rejects.toMatchObject({ kind: "offline" });
    await completeHandshake(lastOf(sockets, DESKTOP_URL), remoteHello({ daemon_id: DESKTOP }));
    await expect(runtime.status(DESKTOP)).resolves.toMatchObject({ version: "0.0.6", protocol: 1 });
  });

  it("tells each ready connection to its listeners", async () => {
    const parts = await setup();
    const ready: MachineId[] = [];
    const stop = parts.runtime.onReady((machine) => ready.push(machine));
    parts.runtime.onReady(() => {
      throw new Error("a listener failed");
    });
    await parts.runtime.load();
    await completeHandshake(lastOf(parts.sockets, LAPTOP_URL), remoteHello({ daemon_id: LAPTOP }));
    await completeHandshake(
      lastOf(parts.sockets, DESKTOP_URL),
      remoteHello({ daemon_id: DESKTOP }),
    );
    expect(ready).toEqual([LAPTOP, DESKTOP]);
    expect(parts.errors).toHaveLength(2);
    stop();
    lastOf(parts.sockets, LAPTOP_URL).drop();
    await parts.clock.advance(1_000);
    await completeHandshake(lastOf(parts.sockets, LAPTOP_URL), remoteHello({ daemon_id: LAPTOP }));
    expect(ready).toEqual([LAPTOP, DESKTOP]);
  });
});

describe("the fan-out of AppState and NetInfo", () => {
  it("gives every machine a new socket after a long time in the background", async () => {
    const { runtime, sockets, lifecycle, clock } = await connected();
    expect(lifecycle.appState.size).toBe(1);
    expect(lifecycle.network.size).toBe(1);
    lifecycle.setActive(false);
    await clock.advance(15_000);
    lifecycle.setActive(true);
    expect(socketsOf(sockets, DESKTOP_URL)).toHaveLength(2);
    expect(socketsOf(sockets, LAPTOP_URL)).toHaveLength(2);
    expect(socketsOf(sockets, DESKTOP_URL)[0]?.closedWith).toBeDefined();
    runtime.dispose();
    expect(lifecycle.appState.size).toBe(0);
    expect(lifecycle.network.size).toBe(0);
  });

  it("keeps the sockets after a short time in the background", async () => {
    const { sockets, lifecycle, clock } = await connected();
    lifecycle.setActive(false);
    await clock.advance(2_000);
    lifecycle.setActive(true);
    expect(sockets.made).toHaveLength(2);
  });

  it("connects every machine again when the network path changes", async () => {
    const { sockets, lifecycle } = await connected();
    lifecycle.setPath({ type: "wifi", connected: true });
    expect(sockets.made).toHaveLength(2);
    lifecycle.setPath({ type: "none", connected: false });
    expect(sockets.made).toHaveLength(2);
    lifecycle.setPath({ type: "cellular", connected: true });
    expect(socketsOf(sockets, DESKTOP_URL)).toHaveLength(2);
    expect(socketsOf(sockets, LAPTOP_URL)).toHaveLength(2);
  });
});

describe("forget", () => {
  it("closes the machine, removes its record and key, and runs the purges", async () => {
    const parts = await connected();
    const { runtime, sockets, devices, binding, purged, cache, streams } = parts;
    runtime.hold({ machine: DESKTOP, id: CONVERSATION }, "follow");
    const laptopHold = runtime.hold({ machine: LAPTOP, id: CONVERSATION }, "follow");
    await flush();

    await runtime.forget(DESKTOP);
    expect(lastOf(sockets, DESKTOP_URL).closedWith).toBeDefined();
    expect(streams.made.map((stream) => [stream.ref.machine, stream.disposed])).toEqual([
      [DESKTOP, true],
      [LAPTOP, false],
    ]);
    expect(runtime.machine(DESKTOP)).toBeUndefined();
    expect(runtime.registry.get().machines.map((machine) => machine.id)).toEqual([LAPTOP]);
    expect(devices.devices.map((device) => device.daemonId)).toEqual([LAPTOP]);
    expect([...binding.keys.keys()]).toEqual([machineRecord(LAPTOP).keyAlias]);
    expect(purged).toEqual([[DESKTOP, cache]]);

    // The other machine keeps its connection and its stream.
    expect(lastOf(sockets, LAPTOP_URL).closedWith).toBeUndefined();
    expect(socketsOf(sockets, LAPTOP_URL)).toHaveLength(1);
    expect(laptopHold?.stream).toMatchObject({ disposed: false });
  });

  it("forgets the last machine", async () => {
    const { runtime, devices } = await connected({ records: [machineRecord(DESKTOP)] });
    await runtime.forget(DESKTOP);
    expect(runtime.registry.get()).toEqual({ loaded: true, machines: [] });
    expect(devices.devices).toEqual([]);
  });

  it("runs every purge even when one fails, and reports the failure", async () => {
    const parts = await setup();
    const failure = new Error("the outbox is locked");
    const runs: string[] = [];
    const runtime = createRuntime({
      pairing: null,
      random: () => 0,
      streams: parts.streams.factory,
      schedule: () => () => {},
      purges: [
        () => Promise.reject(failure),
        () => {
          runs.push("drafts");
        },
      ],
      onError: (error) => parts.errors.push(error),
    });
    await runtime.forget(DESKTOP);
    expect(runs).toEqual(["drafts"]);
    expect(parts.errors).toEqual([failure]);
  });
});

describe("pair", () => {
  it("re-pairs one machine and keeps the connection of the others", async () => {
    const { runtime, sockets, devices, binding } = await connected();
    const laptop = lastOf(sockets, LAPTOP_URL);
    const oldDesktop = lastOf(sockets, DESKTOP_URL);
    const hold = runtime.hold({ machine: DESKTOP, id: CONVERSATION }, "follow");
    await flush();

    const pairing = runtime.pair(DESKTOP_CODE, { name: "iPhone 17 Pro", repair: DESKTOP });
    await flush();
    await confirmPairing(sockets, DESKTOP);
    const machine = await pairing;

    expect(machine).toMatchObject({ id: DESKTOP, name: "desktop", keyMissing: false });
    expect(devices.of(DESKTOP)).toMatchObject({ deviceId: NEW_DEVICE, keyAlias: "device-new-1" });
    expect(devices.devices.map((device) => device.daemonId)).toEqual([DESKTOP, LAPTOP]);
    expect([...binding.keys.keys()]).toEqual([machineRecord(LAPTOP).keyAlias, "device-new-1"]);
    expect(oldDesktop.closedWith).toBeDefined();

    // The new client proves the new device; the followed stream subscribes again.
    const desktop = lastOf(sockets, DESKTOP_URL);
    await completeHandshake(desktop, remoteHello({ daemon_id: DESKTOP }));
    expect(desktop.requests("device.prove")[0]?.["params"]).toMatchObject({
      device_id: NEW_DEVICE,
    });
    expect(desktop.requests("conversation.subscribe")).toHaveLength(1);
    expect(hold?.stream).toMatchObject({ disposed: false });

    expect(laptop.closedWith).toBeUndefined();
    expect(socketsOf(sockets, LAPTOP_URL)).toHaveLength(1);
    expect(runtime.registry.get().machines.map((one) => one.id)).toEqual([DESKTOP, LAPTOP]);
  });

  it("adds a new machine and connects it", async () => {
    const { runtime, sockets, devices } = await connected({ records: [machineRecord(LAPTOP)] });
    const pairing = runtime.pair(DESKTOP_CODE, { name: "iPhone 17 Pro" });
    await flush();
    await confirmPairing(sockets, DESKTOP);
    await expect(pairing).resolves.toMatchObject({ id: DESKTOP });
    expect(runtime.registry.get().machines.map((one) => one.name)).toEqual(["laptop", "desktop"]);
    expect(devices.devices.map((device) => device.daemonId)).toEqual([LAPTOP, DESKTOP]);
    const desktop = lastOf(sockets, DESKTOP_URL);
    await completeHandshake(desktop, remoteHello({ daemon_id: DESKTOP }));
    expect(runtime.machine(DESKTOP)?.connection.get().view.kind).toBe("online");
    expect(lastOf(sockets, LAPTOP_URL).closedWith).toBeUndefined();
  });

  it("rejects with the failure of the flow, and keeps every machine", async () => {
    const { runtime, sockets, devices } = await connected();
    const other = runtime.pair(DESKTOP_CODE, { name: "iPhone", repair: LAPTOP });
    await expect(other).rejects.toMatchObject({
      failure: { kind: "other_machine", daemonId: DESKTOP, fqdn: "desktop.tail3f2a.ts.net" },
    });
    const garbage = runtime.pair("https://example.com", { name: "iPhone" });
    await expect(garbage).rejects.toMatchObject({ failure: { kind: "not_efr" } });
    expect(sockets.made).toHaveLength(2);
    expect(devices.devices).toHaveLength(2);
  });
});
