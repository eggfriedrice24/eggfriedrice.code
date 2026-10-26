// Fakes for the tests of pairing. Only test files import this module.

import { base64Encode } from "../protocol/bytes";
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
