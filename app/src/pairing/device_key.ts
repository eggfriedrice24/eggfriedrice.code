// The device key: a P-256 key that proves this phone to efrd (R3.7 of the research).
// On an iPhone it lives in the Secure Enclave, through the local module
// app/modules/efr-device-key. This file holds the interface that the rest of the app
// uses and an adapter over the native module that checks what the module gives back.
//
// The adapter takes the native functions as a parameter, so it runs in Vitest with a fake.

import { decodeBase64, encodeBase64 } from "./base64";

/** One device key. The private key never leaves the key store. */
export interface DeviceKey {
  /** The name of the key in the key store, which the pairing record keeps. */
  readonly alias: string;
  /** The 65-byte uncompressed public key: 0x04, then X and Y (X9.63). */
  readonly publicKey: Uint8Array;
  /** The ECDSA P-256 signature over the SHA-256 of `message`, in DER. */
  sign(message: Uint8Array): Promise<Uint8Array>;
}

/** Where the device keys live. */
export interface DeviceKeyStore {
  /** True for the Secure Enclave, false for the software key of the Simulator. */
  readonly hardwareBacked: boolean;
  /** Makes a new key under `alias` in place of any old one. */
  create(alias: string): Promise<DeviceKey>;
  /** The key under `alias`, or null when there is none. */
  load(alias: string): Promise<DeviceKey | null>;
  /** Removes the key under `alias`. A missing key is not an error. */
  remove(alias: string): Promise<void>;
}

/** The native functions, as app/modules/efr-device-key gives them (base64 strings). */
export interface DeviceKeyBinding {
  isHardwareBacked(): boolean;
  createKey(alias: string): Promise<string>;
  publicKey(alias: string): Promise<string | null>;
  sign(alias: string, message: string): Promise<string>;
  deleteKey(alias: string): Promise<void>;
}

/** The key store failed, or gave back a value of the wrong shape. */
export class DeviceKeyError extends Error {
  override readonly name = "DeviceKeyError";
}

/** The length of an X9.63 P-256 public key. */
export const PUBLIC_KEY_BYTES = 65;

const ALIAS = /^[A-Za-z0-9._-]{1,64}$/;

/** A key store over the native module. */
export function deviceKeyStore(binding: DeviceKeyBinding): DeviceKeyStore {
  const key = (alias: string, publicKey: Uint8Array): DeviceKey => ({
    alias,
    publicKey,
    async sign(message: Uint8Array): Promise<Uint8Array> {
      const signature = decodeBase64(await binding.sign(alias, encodeBase64(message)));
      if (signature === undefined || !isDerSignature(signature)) {
        throw new DeviceKeyError("the device key gave a signature that is not DER");
      }
      return signature;
    },
  });
  return {
    hardwareBacked: binding.isHardwareBacked(),
    async create(alias: string): Promise<DeviceKey> {
      checkAlias(alias);
      return key(alias, publicKeyFrom(await binding.createKey(alias)));
    },
    async load(alias: string): Promise<DeviceKey | null> {
      checkAlias(alias);
      const text = await binding.publicKey(alias);
      return text === null ? null : key(alias, publicKeyFrom(text));
    },
    async remove(alias: string): Promise<void> {
      checkAlias(alias);
      await binding.deleteKey(alias);
    },
  };
}

function checkAlias(alias: string): void {
  if (!ALIAS.test(alias)) {
    throw new DeviceKeyError("a key alias is 1 to 64 ASCII letters, digits, '.', '_' or '-'");
  }
}

function publicKeyFrom(text: string): Uint8Array {
  const bytes = decodeBase64(text);
  if (!isPublicKey(bytes)) {
    throw new DeviceKeyError("the device key gave a public key that is not a P-256 point");
  }
  return bytes;
}

/** True for 65 bytes that start with 0x04, the form that `device.enroll` takes. */
export function isPublicKey(bytes: Uint8Array | undefined): bytes is Uint8Array {
  return bytes !== undefined && bytes.length === PUBLIC_KEY_BYTES && bytes[0] === 0x04;
}

/**
 * True when `bytes` has the shape of a DER ECDSA P-256 signature: a SEQUENCE of two
 * positive INTEGERs of at most 33 bytes each, in minimal form. It does not check the
 * values; efrd verifies the signature.
 */
export function isDerSignature(bytes: Uint8Array): boolean {
  if (bytes.length < 8 || bytes[0] !== 0x30 || bytes[1] !== bytes.length - 2) {
    return false;
  }
  let at = 2;
  for (let n = 0; n < 2; n++) {
    const tag = bytes[at];
    const length = bytes[at + 1];
    if (tag !== 0x02 || length === undefined || length < 1 || length > 33) {
      return false;
    }
    const first = bytes[at + 2] ?? 0;
    const second = bytes[at + 3];
    // A negative value, or a zero byte that the next byte does not need.
    if ((first & 0x80) !== 0 || (first === 0 && length > 1 && ((second ?? 0) & 0x80) === 0)) {
      return false;
    }
    at += 2 + length;
  }
  return at === bytes.length;
}

/**
 * The fingerprint of a public key that efrd shows at the terminal and the phone shows
 * while it waits: `SHA256:` and the first 8 bytes of the SHA-256 of the 65-byte key in
 * lowercase hex (the form of the fixture `device_pair_start_item_pending.json`).
 */
export function keyFingerprint(sha256: Uint8Array): string {
  if (sha256.length !== 32) {
    throw new DeviceKeyError("a SHA-256 digest has 32 bytes");
  }
  let hex = "";
  for (const byte of sha256.subarray(0, 8)) {
    hex += byte.toString(16).padStart(2, "0");
  }
  return `SHA256:${hex}`;
}
