// The pairing records: what the phone keeps after efrd confirms it, one record for each
// machine. A record holds the device id, the daemon and where to find it, and the alias of
// the device key. It holds no secret (the token is spent and the private key stays in the
// key store), but it goes into expo-secure-store with WHEN_UNLOCKED_THIS_DEVICE_ONLY, so
// that it is not in a backup and does not go to a new phone, where the key would be absent.
//
// expo-secure-store cannot list its keys, so all records are one JSON list (version 2)
// under one key. Version 1 kept one record under another key; the first read moves it into
// the list and then removes the old key.

import type { DeviceKey, DeviceKeyStore } from "./device-key";
import {
  RECORDS_VERSION,
  storedRecordSchema,
  storedRecordsSchema,
  storedRecordV1Schema,
  type StoredRecord,
} from "./paired-device-schemas";

/** One pairing with one daemon. */
export interface PairedDevice {
  /** The device id that `device.enroll` gave. */
  readonly deviceId: string;
  /** The daemon that the phone paired with; `hello` must give the same. */
  readonly daemonId: string;
  /** The daemon's tailnet IPv4 address. */
  readonly host: string;
  readonly port: number;
  /** The MagicDNS name of the daemon's node, when the pairing code had one. */
  readonly fqdn?: string;
  /** The name that the phone sent, which `efr device list` shows. */
  readonly name: string;
  /** The alias of the device key in the key store. */
  readonly keyAlias: string;
  /** What the device may do, from the enroll result. */
  readonly scopes: readonly string[];
  /** When the terminal confirmed the pairing, as an RFC 3339 time. */
  readonly pairedAt: string;
}

/** Where the pairing records live. */
export interface PairedDeviceStore {
  /** Every record, in the order of the first pairing with each machine. */
  loadAll(): Promise<readonly PairedDevice[]>;
  /**
   * Keeps `device`. It replaces the record with the same `daemonId` (a re-pair) in its
   * place, and adds a record for a new machine at the end.
   */
  upsert(device: PairedDevice): Promise<void>;
  /** Removes the record of daemon `daemonId`. A missing record is not an error. */
  remove(daemonId: string): Promise<void>;
}

/** The functions of expo-secure-store that the record store uses. */
export interface SecureStoreApi {
  getItemAsync(key: string, options: SecureStoreOptions): Promise<string | null>;
  setItemAsync(key: string, value: string, options: SecureStoreOptions): Promise<void>;
  deleteItemAsync(key: string, options: SecureStoreOptions): Promise<void>;
}

/** The options of expo-secure-store that the record store sets. */
export interface SecureStoreOptions {
  readonly keychainService: string;
  /** expo-secure-store's `WHEN_UNLOCKED_THIS_DEVICE_ONLY`. */
  readonly keychainAccessible: number;
}

/** The expo-secure-store key of the list of records (version 2). */
export const PAIRED_DEVICES_KEY = "efr.paired_devices";

/** The expo-secure-store key of the single record of version 1. */
export const PAIRED_DEVICE_V1_KEY = "efr.paired_device";

/** The keychain service of the records. */
export const PAIRED_DEVICE_SERVICE = "dev.eggfriedrice.efr.pairing";

/**
 * A record store over expo-secure-store. `whenUnlockedThisDeviceOnly` is the value of
 * expo-secure-store's `WHEN_UNLOCKED_THIS_DEVICE_ONLY`.
 *
 * The store runs one operation at a time, because each change reads and writes the whole
 * list.
 */
export function secureStorePairedDevices(
  api: SecureStoreApi,
  whenUnlockedThisDeviceOnly: number,
): PairedDeviceStore {
  const options: SecureStoreOptions = {
    keychainService: PAIRED_DEVICE_SERVICE,
    keychainAccessible: whenUnlockedThisDeviceOnly,
  };
  let queue: Promise<unknown> = Promise.resolve();
  const serial = <T>(run: () => Promise<T>): Promise<T> => {
    const next = queue.then(run, run);
    queue = next.catch(() => undefined);
    return next;
  };

  const write = async (devices: readonly PairedDevice[]): Promise<void> => {
    if (devices.length === 0) {
      await api.deleteItemAsync(PAIRED_DEVICES_KEY, options);
    } else {
      await api.setItemAsync(PAIRED_DEVICES_KEY, encodePairedDevices(devices), options);
    }
  };

  const read = async (): Promise<readonly PairedDevice[]> => {
    const text = await api.getItemAsync(PAIRED_DEVICES_KEY, options);
    const old = await api.getItemAsync(PAIRED_DEVICE_V1_KEY, options);
    if (old === null) {
      return text === null ? [] : decodePairedDevices(text);
    }
    if (text !== null) {
      // NOTE: an earlier move wrote the list, then stopped before it removed the old key.
      await api.deleteItemAsync(PAIRED_DEVICE_V1_KEY, options);
      return decodePairedDevices(text);
    }
    const migrated = decodePairedDeviceV1(old);
    const devices = migrated === null ? [] : [migrated];
    if (devices.length > 0) {
      await write(devices);
    }
    await api.deleteItemAsync(PAIRED_DEVICE_V1_KEY, options);
    return devices;
  };

  return {
    loadAll: () => serial(read),
    upsert: (device) =>
      serial(async () => {
        await write(withDevice(await read(), device));
      }),
    remove: (daemonId) =>
      serial(async () => {
        const devices = await read();
        const kept = devices.filter((device) => device.daemonId !== daemonId);
        if (kept.length !== devices.length) {
          await write(kept);
        }
      }),
  };
}

/** `devices` with `device` in place of the record of the same daemon, or added at the end. */
export function withDevice(
  devices: readonly PairedDevice[],
  device: PairedDevice,
): readonly PairedDevice[] {
  let replaced = false;
  const next = devices.map((old) => {
    if (old.daemonId !== device.daemonId) {
      return old;
    }
    replaced = true;
    return device;
  });
  if (!replaced) {
    next.push(device);
  }
  return next;
}

/** The stored JSON of the list (version 2). */
export function encodePairedDevices(devices: readonly PairedDevice[]): string {
  return JSON.stringify({ version: RECORDS_VERSION, records: devices.map(storedRecord) });
}

/**
 * The records in `text`. A text that is not a list of version 2 gives no records, and the
 * app treats the phone as not paired. A bad record is left out; the others stay.
 */
export function decodePairedDevices(text: string): readonly PairedDevice[] {
  const parsed = storedRecordsSchema.safeParse(parseJson(text));
  if (!parsed.success) {
    return [];
  }
  const devices: PairedDevice[] = [];
  for (const value of parsed.data.records) {
    const record = storedRecordSchema.safeParse(value);
    if (record.success && !devices.some((d) => d.daemonId === record.data.daemon_id)) {
      devices.push(pairedDevice(record.data));
    }
  }
  return devices;
}

/** The record of version 1 in `text`, or null when it is not one. */
export function decodePairedDeviceV1(text: string): PairedDevice | null {
  const parsed = storedRecordV1Schema.safeParse(parseJson(text));
  return parsed.success ? pairedDevice(parsed.data) : null;
}

function storedRecord(device: PairedDevice): StoredRecord {
  const record: StoredRecord = {
    device_id: device.deviceId,
    daemon_id: device.daemonId,
    host: device.host,
    port: device.port,
    name: device.name,
    key_alias: device.keyAlias,
    scopes: [...device.scopes],
    paired_at: device.pairedAt,
  };
  if (device.fqdn !== undefined) {
    record.fqdn = device.fqdn;
  }
  return record;
}

function pairedDevice(record: StoredRecord): PairedDevice {
  const device: PairedDevice = {
    deviceId: record.device_id,
    daemonId: record.daemon_id,
    host: record.host,
    port: record.port,
    name: record.name,
    keyAlias: record.key_alias,
    scopes: record.scopes,
    pairedAt: record.paired_at,
  };
  return record.fqdn === undefined ? device : { ...device, fqdn: record.fqdn };
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

/**
 * The short name of a machine, such as `desktop`: the first label of its MagicDNS name, or
 * its tailnet address when the pairing code had no name.
 */
export function machineName(target: { readonly host: string; readonly fqdn?: string }): string {
  const label = target.fqdn?.split(".")[0] ?? "";
  return label === "" ? target.host : label;
}

/**
 * The WebSocket URL of the daemon: its tailnet IPv4 address and port, at path `/`. The
 * app never dials the MagicDNS name, because efrd accepts only the Origin
 * `http://<tailnet IPv4>:<port>`, which React Native sets from the URL.
 */
export function daemonUrl(target: { readonly host: string; readonly port: number }): string {
  return `ws://${target.host}:${target.port}/`;
}

/**
 * A pairing record and its device key: what the client needs to connect and prove.
 * `key` is null when the key store lost the key (the app moved to another phone, or the
 * keychain lost it): the machine stays in the list, and the user must pair it again.
 */
export interface Pairing {
  readonly device: PairedDevice;
  readonly key: DeviceKey | null;
}

/** Every pairing of this phone, with its key when the key store still has it. */
export async function loadPairings(
  keys: DeviceKeyStore,
  devices: PairedDeviceStore,
): Promise<readonly Pairing[]> {
  const records = await devices.loadAll();
  return Promise.all(
    records.map(async (device) => ({
      device,
      key: await keys.load(device.keyAlias).catch(() => null),
    })),
  );
}

/** Removes the record of daemon `daemonId` and its key, for "Forget this machine". */
export async function forgetPairing(
  keys: DeviceKeyStore,
  devices: PairedDeviceStore,
  daemonId: string,
): Promise<void> {
  const device = (await devices.loadAll()).find((record) => record.daemonId === daemonId);
  await devices.remove(daemonId);
  if (device !== undefined) {
    await keys.remove(device.keyAlias);
  }
}

/** `value` as an array of strings, or undefined when it is not one. */
export function stringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const strings: string[] = [];
  for (const item of value as readonly unknown[]) {
    if (typeof item !== "string") {
      return undefined;
    }
    strings.push(item);
  }
  return strings;
}
