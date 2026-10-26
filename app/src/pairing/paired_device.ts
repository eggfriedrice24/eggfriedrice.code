// The pairing record: what the phone keeps after efrd confirms it. It holds the device id,
// the daemon and where to find it, and the alias of the device key. It holds no secret
// (the token is spent and the private key stays in the key store), but it goes into
// expo-secure-store with WHEN_UNLOCKED_THIS_DEVICE_ONLY, so that it is not in a backup
// and does not go to a new phone, where the key would be absent.
//
// v1 pairs the phone with one daemon. A new pairing replaces the record.

import type { DeviceKey, DeviceKeyStore } from "./device_key";
import { isJsonObject, stringArray } from "./json";
import { isTailnetIpv4 } from "./pairing_url";

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

/** Where the pairing record lives. */
export interface PairedDeviceStore {
  /** The record, or null when the phone is not paired. */
  load(): Promise<PairedDevice | null>;
  save(device: PairedDevice): Promise<void>;
  remove(): Promise<void>;
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

/** The expo-secure-store key of the record. */
export const PAIRED_DEVICE_KEY = "efr.paired_device";

/** The keychain service of the record. */
export const PAIRED_DEVICE_SERVICE = "dev.eggfriedrice.efr.pairing";

/** The version of the stored JSON. */
const RECORD_VERSION = 1;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const ALIAS = /^[A-Za-z0-9._-]{1,64}$/;

/**
 * A record store over expo-secure-store. `whenUnlockedThisDeviceOnly` is the value of
 * expo-secure-store's `WHEN_UNLOCKED_THIS_DEVICE_ONLY`.
 */
export function secureStorePairedDevices(
  api: SecureStoreApi,
  whenUnlockedThisDeviceOnly: number,
): PairedDeviceStore {
  const options: SecureStoreOptions = {
    keychainService: PAIRED_DEVICE_SERVICE,
    keychainAccessible: whenUnlockedThisDeviceOnly,
  };
  return {
    async load(): Promise<PairedDevice | null> {
      const text = await api.getItemAsync(PAIRED_DEVICE_KEY, options);
      return text === null ? null : decodePairedDevice(text);
    },
    async save(device: PairedDevice): Promise<void> {
      await api.setItemAsync(PAIRED_DEVICE_KEY, encodePairedDevice(device), options);
    },
    async remove(): Promise<void> {
      await api.deleteItemAsync(PAIRED_DEVICE_KEY, options);
    },
  };
}

/** The stored JSON of a record. */
export function encodePairedDevice(device: PairedDevice): string {
  return JSON.stringify({
    version: RECORD_VERSION,
    device_id: device.deviceId,
    daemon_id: device.daemonId,
    host: device.host,
    port: device.port,
    ...(device.fqdn === undefined ? {} : { fqdn: device.fqdn }),
    name: device.name,
    key_alias: device.keyAlias,
    scopes: device.scopes,
    paired_at: device.pairedAt,
  });
}

/**
 * The record in `text`, or null when it is not a record of this version. The app then
 * treats the phone as not paired, and the user pairs again.
 */
export function decodePairedDevice(text: string): PairedDevice | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isJsonObject(value)) {
    return null;
  }
  const {
    version,
    device_id: deviceId,
    daemon_id: daemonId,
    host,
    port,
    fqdn,
    name,
    key_alias: keyAlias,
    scopes,
    paired_at: pairedAt,
  } = value;
  const scopeList = stringArray(scopes);
  if (
    version !== RECORD_VERSION ||
    typeof deviceId !== "string" ||
    !UUID.test(deviceId) ||
    typeof daemonId !== "string" ||
    !UUID.test(daemonId) ||
    typeof host !== "string" ||
    !isTailnetIpv4(host) ||
    typeof port !== "number" ||
    !Number.isInteger(port) ||
    port < 1 ||
    port > 65_535 ||
    (fqdn !== undefined && typeof fqdn !== "string") ||
    typeof name !== "string" ||
    typeof keyAlias !== "string" ||
    !ALIAS.test(keyAlias) ||
    scopeList === undefined ||
    typeof pairedAt !== "string"
  ) {
    return null;
  }
  const device: PairedDevice = {
    deviceId,
    daemonId,
    host,
    port,
    name,
    keyAlias,
    scopes: scopeList,
    pairedAt,
  };
  return typeof fqdn === "string" ? { ...device, fqdn } : device;
}

/**
 * The WebSocket URL of the daemon: its tailnet IPv4 address and port, at path `/`. The
 * app never dials the MagicDNS name, because efrd accepts only the Origin
 * `http://<tailnet IPv4>:<port>`, which React Native sets from the URL.
 */
export function daemonUrl(target: { readonly host: string; readonly port: number }): string {
  return `ws://${target.host}:${target.port}/`;
}

/** A pairing record and its device key: what the client needs to connect and prove. */
export interface Pairing {
  readonly device: PairedDevice;
  readonly key: DeviceKey;
}

/**
 * The pairing of this phone, or null when it is not paired. A record whose key is gone
 * (the app moved to another phone, or the keychain lost it) is no pairing: the user must
 * pair again.
 */
export async function loadPairing(
  keys: DeviceKeyStore,
  devices: PairedDeviceStore,
): Promise<Pairing | null> {
  const device = await devices.load();
  if (device === null) {
    return null;
  }
  const key = await keys.load(device.keyAlias);
  return key === null ? null : { device, key };
}

/** Removes the pairing record and its key, for "re-pair" and after a revoke. */
export async function forgetPairing(
  keys: DeviceKeyStore,
  devices: PairedDeviceStore,
): Promise<void> {
  const device = await devices.load();
  await devices.remove();
  if (device !== null) {
    await keys.remove(device.keyAlias);
  }
}
