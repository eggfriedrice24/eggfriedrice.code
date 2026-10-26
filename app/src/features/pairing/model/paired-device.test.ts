import { describe, expect, it } from "vitest";

import { deviceKeyStore } from "./device-key";
import {
  daemonUrl,
  decodePairedDevices,
  decodePairedDeviceV1,
  encodePairedDevices,
  forgetPairing,
  loadPairings,
  machineName,
  PAIRED_DEVICE_SERVICE,
  PAIRED_DEVICE_V1_KEY,
  PAIRED_DEVICES_KEY,
  secureStorePairedDevices,
} from "./paired-device";
import type { PairedDevice, SecureStoreApi, SecureStoreOptions } from "./paired-device";
import { FakeKeyBinding, MemoryPairedDevices } from "./testing";

const DEVICE: PairedDevice = {
  deviceId: "019a9b1c-3d00-7a10-8b20-000000000006",
  daemonId: "019a9b1c-3d00-7a10-8b20-000000000007",
  host: "100.101.102.103",
  port: 7878,
  fqdn: "desk.tail1234.ts.net",
  name: "Anna's iPhone",
  keyAlias: "device-1",
  scopes: ["read", "operate", "approve"],
  pairedAt: "2026-10-26T09:01:00.000Z",
};

const LAPTOP: PairedDevice = {
  deviceId: "019a9b1c-3d00-7a10-8b20-0000000000c1",
  daemonId: "019a9b1c-3d00-7a10-8b20-0000000000c2",
  host: "100.64.0.9",
  port: 7419,
  name: "Anna's iPhone",
  keyAlias: "device-2",
  scopes: ["read"],
  pairedAt: "2026-10-26T10:00:00.000Z",
};

/** The stored JSON of `DEVICE` in version 1, as the app of v0.0.5 wrote it. */
const V1_TEXT = JSON.stringify({
  version: 1,
  device_id: DEVICE.deviceId,
  daemon_id: DEVICE.daemonId,
  host: DEVICE.host,
  port: DEVICE.port,
  fqdn: DEVICE.fqdn,
  name: DEVICE.name,
  key_alias: DEVICE.keyAlias,
  scopes: DEVICE.scopes,
  paired_at: DEVICE.pairedAt,
});

/** expo-secure-store in memory, which records the options of each call. */
class FakeSecureStore implements SecureStoreApi {
  readonly items = new Map<string, string>();
  readonly options: SecureStoreOptions[] = [];

  getItemAsync(key: string, options: SecureStoreOptions): Promise<string | null> {
    this.options.push(options);
    return Promise.resolve(this.items.get(key) ?? null);
  }

  setItemAsync(key: string, value: string, options: SecureStoreOptions): Promise<void> {
    this.options.push(options);
    this.items.set(key, value);
    return Promise.resolve();
  }

  deleteItemAsync(key: string, options: SecureStoreOptions): Promise<void> {
    this.options.push(options);
    this.items.delete(key);
    return Promise.resolve();
  }
}

/** The stored record of `device` in version 2. */
function stored(device: PairedDevice): Record<string, unknown> {
  const record: Record<string, unknown> = {
    device_id: device.deviceId,
    daemon_id: device.daemonId,
    host: device.host,
    port: device.port,
    name: device.name,
    key_alias: device.keyAlias,
    scopes: device.scopes,
    paired_at: device.pairedAt,
  };
  if (device.fqdn !== undefined) {
    record["fqdn"] = device.fqdn;
  }
  return record;
}

function storedDevice(): Record<string, unknown> {
  return stored(DEVICE);
}

describe("the pairing records", () => {
  it("give back what they stored, with and without a MagicDNS name", () => {
    expect(decodePairedDevices(encodePairedDevices([DEVICE, LAPTOP]))).toEqual([DEVICE, LAPTOP]);
    expect(decodePairedDevices(encodePairedDevices([]))).toEqual([]);
  });

  it("store one snake_case list with a version", () => {
    expect(JSON.parse(encodePairedDevices([DEVICE]))).toEqual({
      version: 2,
      records: [storedDevice()],
    });
  });

  it("leave out a record of another shape and keep the others", () => {
    const good = storedDevice();
    const bad: unknown[] = [
      { ...good, device_id: "nope" },
      { ...good, daemon_id: 7 },
      { ...good, host: "127.0.0.1" },
      { ...good, port: 0 },
      { ...good, port: 1.5 },
      { ...good, fqdn: 3 },
      { ...good, key_alias: "a/b" },
      { ...good, scopes: ["read", 1] },
      { ...good, scopes: "read" },
      { ...good, paired_at: undefined },
      [good],
      null,
    ];
    for (const value of bad) {
      const text = JSON.stringify({ version: 2, records: [value, stored(LAPTOP)] });
      expect({ value, devices: decodePairedDevices(text) }).toEqual({ value, devices: [LAPTOP] });
    }
  });

  it("keep the first record of a daemon that is in the list twice", () => {
    const text = JSON.stringify({
      version: 2,
      records: [storedDevice(), { ...storedDevice(), key_alias: "device-9" }],
    });
    expect(decodePairedDevices(text)).toEqual([DEVICE]);
  });

  it("treat a list of another version or shape as no pairing", () => {
    for (const text of [
      "{",
      "null",
      JSON.stringify({ version: 3, records: [storedDevice()] }),
      JSON.stringify({ version: 2, records: storedDevice() }),
      JSON.stringify([storedDevice()]),
      V1_TEXT,
    ]) {
      expect({ text, devices: decodePairedDevices(text) }).toEqual({ text, devices: [] });
    }
  });

  it("read the record of version 1", () => {
    expect(decodePairedDeviceV1(V1_TEXT)).toEqual(DEVICE);
    expect(decodePairedDeviceV1(JSON.stringify({ ...storedDevice(), version: 2 }))).toBeNull();
    expect(decodePairedDeviceV1("{")).toBeNull();
  });
});

describe("secureStorePairedDevices", () => {
  it("keeps the records in its own keychain service, for this device only", async () => {
    const api = new FakeSecureStore();
    const store = secureStorePairedDevices(api, 42);
    expect(await store.loadAll()).toEqual([]);
    await store.upsert(DEVICE);
    await store.upsert(LAPTOP);
    expect(api.items.has(PAIRED_DEVICES_KEY)).toBe(true);
    expect(await store.loadAll()).toEqual([DEVICE, LAPTOP]);
    await store.remove(DEVICE.daemonId);
    expect(await store.loadAll()).toEqual([LAPTOP]);
    await store.remove(LAPTOP.daemonId);
    expect(await store.loadAll()).toEqual([]);
    expect(api.items.size).toBe(0);
    for (const options of api.options) {
      expect(options).toEqual({ keychainService: PAIRED_DEVICE_SERVICE, keychainAccessible: 42 });
    }
  });

  it("replaces only the record of the same daemon, in its place", async () => {
    const store = secureStorePairedDevices(new FakeSecureStore(), 42);
    await store.upsert(DEVICE);
    await store.upsert(LAPTOP);
    const repaired = { ...DEVICE, keyAlias: "device-3", deviceId: LAPTOP.deviceId };
    await store.upsert(repaired);
    expect(await store.loadAll()).toEqual([repaired, LAPTOP]);
  });

  it("runs one change at a time, so two changes at once both stay", async () => {
    const store = secureStorePairedDevices(new FakeSecureStore(), 42);
    await Promise.all([store.upsert(DEVICE), store.upsert(LAPTOP)]);
    expect(await store.loadAll()).toEqual([DEVICE, LAPTOP]);
  });

  it("moves the record of version 1 into the list, then removes the old key", async () => {
    const api = new FakeSecureStore();
    api.items.set(PAIRED_DEVICE_V1_KEY, V1_TEXT);
    const store = secureStorePairedDevices(api, 42);
    expect(await store.loadAll()).toEqual([DEVICE]);
    expect(api.items.has(PAIRED_DEVICE_V1_KEY)).toBe(false);
    expect(decodePairedDevices(api.items.get(PAIRED_DEVICES_KEY) ?? "")).toEqual([DEVICE]);
    // A new store, as after a restart, reads the list.
    expect(await secureStorePairedDevices(api, 42).loadAll()).toEqual([DEVICE]);
  });

  it("adds a new machine to a moved record", async () => {
    const api = new FakeSecureStore();
    api.items.set(PAIRED_DEVICE_V1_KEY, V1_TEXT);
    const store = secureStorePairedDevices(api, 42);
    await store.upsert(LAPTOP);
    expect(await store.loadAll()).toEqual([DEVICE, LAPTOP]);
    expect(api.items.has(PAIRED_DEVICE_V1_KEY)).toBe(false);
  });

  it("removes a bad record of version 1 and gives no pairing", async () => {
    const api = new FakeSecureStore();
    api.items.set(PAIRED_DEVICE_V1_KEY, "{");
    expect(await secureStorePairedDevices(api, 42).loadAll()).toEqual([]);
    expect(api.items.size).toBe(0);
  });

  it("keeps the list when a move stopped before it removed the old key", async () => {
    const api = new FakeSecureStore();
    api.items.set(PAIRED_DEVICES_KEY, encodePairedDevices([DEVICE, LAPTOP]));
    api.items.set(PAIRED_DEVICE_V1_KEY, V1_TEXT);
    expect(await secureStorePairedDevices(api, 42).loadAll()).toEqual([DEVICE, LAPTOP]);
    expect(api.items.has(PAIRED_DEVICE_V1_KEY)).toBe(false);
  });
});

describe("loadPairings and forgetPairing", () => {
  it("give each record with its key, and remove one machine with its key", async () => {
    const binding = new FakeKeyBinding();
    const keys = deviceKeyStore(binding);
    const devices = new MemoryPairedDevices();
    expect(await loadPairings(keys, devices)).toEqual([]);

    const key = await keys.create(DEVICE.keyAlias);
    await keys.create(LAPTOP.keyAlias);
    devices.devices = [DEVICE, LAPTOP];
    const pairings = await loadPairings(keys, devices);
    expect(pairings.map((pairing) => pairing.device)).toEqual([DEVICE, LAPTOP]);
    expect(pairings[0]?.key?.publicKey).toEqual(key.publicKey);

    await forgetPairing(keys, devices, DEVICE.daemonId);
    expect(devices.devices).toEqual([LAPTOP]);
    expect([...binding.keys.keys()]).toEqual([LAPTOP.keyAlias]);
    // A machine that is not paired is no error.
    await forgetPairing(keys, devices, DEVICE.daemonId);
    expect(devices.devices).toEqual([LAPTOP]);
  });

  it("keep a record whose key is gone, without a key", async () => {
    const devices = new MemoryPairedDevices([DEVICE]);
    const pairings = await loadPairings(deviceKeyStore(new FakeKeyBinding()), devices);
    expect(pairings).toEqual([{ device: DEVICE, key: null }]);
  });
});

describe("machineName", () => {
  it("is the first label of the MagicDNS name, else the address", () => {
    expect(machineName(DEVICE)).toBe("desk");
    expect(machineName(LAPTOP)).toBe("100.64.0.9");
  });
});

describe("daemonUrl", () => {
  it("is the tailnet address, never the MagicDNS name", () => {
    expect(daemonUrl(DEVICE)).toBe("ws://100.101.102.103:7878/");
    expect(daemonUrl({ host: "100.64.0.1", port: 7419 })).toBe("ws://100.64.0.1:7419/");
  });
});
