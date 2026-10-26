import { describe, expect, it } from "vitest";

import { deviceKeyStore } from "./device_key";
import { isJsonObject } from "./json";
import {
  daemonUrls,
  decodePairedDevice,
  encodePairedDevice,
  forgetPairing,
  loadPairing,
  PAIRED_DEVICE_KEY,
  PAIRED_DEVICE_SERVICE,
  secureStorePairedDevices,
} from "./paired_device";
import type { PairedDevice, SecureStoreApi, SecureStoreOptions } from "./paired_device";
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

describe("the pairing record", () => {
  it("gives back what it stored, with and without a MagicDNS name", () => {
    expect(decodePairedDevice(encodePairedDevice(DEVICE))).toEqual(DEVICE);
    const { fqdn: _, ...withoutName } = DEVICE;
    expect(decodePairedDevice(encodePairedDevice(withoutName))).toEqual(withoutName);
  });

  it("stores snake_case JSON with a version", () => {
    expect(JSON.parse(encodePairedDevice(DEVICE))).toEqual({
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
  });

  it("treats a record of another version or shape as no pairing", () => {
    const good: unknown = JSON.parse(encodePairedDevice(DEVICE));
    if (!isJsonObject(good)) {
      throw new Error("the record is not an object");
    }
    const bad: unknown[] = [
      { ...good, version: 2 },
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
      const text = JSON.stringify(value);
      expect({ text, device: decodePairedDevice(text) }).toEqual({ text, device: null });
    }
    expect(decodePairedDevice("{")).toBeNull();
  });
});

describe("secureStorePairedDevices", () => {
  it("keeps the record in its own keychain service, for this device only", async () => {
    const api = new FakeSecureStore();
    const store = secureStorePairedDevices(api, 42);
    expect(await store.load()).toBeNull();
    await store.save(DEVICE);
    expect(api.items.has(PAIRED_DEVICE_KEY)).toBe(true);
    expect(await store.load()).toEqual(DEVICE);
    await store.remove();
    expect(await store.load()).toBeNull();
    expect(api.options.length).toBe(5);
    for (const options of api.options) {
      expect(options).toEqual({ keychainService: PAIRED_DEVICE_SERVICE, keychainAccessible: 42 });
    }
  });
});

describe("loadPairing and forgetPairing", () => {
  it("give the record with its key, and remove both", async () => {
    const binding = new FakeKeyBinding();
    const keys = deviceKeyStore(binding);
    const devices = new MemoryPairedDevices();
    expect(await loadPairing(keys, devices)).toBeNull();

    const key = await keys.create("device-1");
    devices.device = DEVICE;
    const pairing = await loadPairing(keys, devices);
    expect(pairing?.device).toEqual(DEVICE);
    expect(pairing?.key.publicKey).toEqual(key.publicKey);

    await forgetPairing(keys, devices);
    expect(devices.device).toBeNull();
    expect(binding.keys.size).toBe(0);
  });

  it("treat a record whose key is gone as no pairing", async () => {
    const devices = new MemoryPairedDevices();
    devices.device = DEVICE;
    expect(await loadPairing(deviceKeyStore(new FakeKeyBinding()), devices)).toBeNull();
  });
});

describe("daemonUrls", () => {
  it("tries the tailnet address first, then the MagicDNS name", () => {
    expect(daemonUrls(DEVICE)).toEqual([
      "ws://100.101.102.103:7878/",
      "ws://desk.tail1234.ts.net:7878/",
    ]);
    expect(daemonUrls({ host: "100.64.0.1", port: 7419 })).toEqual(["ws://100.64.0.1:7419/"]);
  });
});
