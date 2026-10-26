// The functions of the native module EfrDeviceKey (ios/EfrDeviceKeyModule.swift). Every
// byte value crosses as standard base64. Only this file and index.ts know the native
// names; the app uses the DeviceKeyStore of src/features/pairing/model/device-key.ts.

/** The native module, as `requireOptionalNativeModule("EfrDeviceKey")` gives it. */
export interface EfrDeviceKeyNative {
  /** True when the key is in the Secure Enclave, false for the Simulator's software key. */
  isHardwareBacked(): boolean;
  /** Makes a new key under `alias` in place of any old one. Gives the public key. */
  createKey(alias: string): Promise<string>;
  /** The public key under `alias` (65 bytes, X9.63), or null when there is no key. */
  publicKey(alias: string): Promise<string | null>;
  /** The DER signature (ECDSA P-256, SHA-256) of `message` with the key under `alias`. */
  sign(alias: string, message: string): Promise<string>;
  /** Removes the key under `alias`. A missing key is not an error. */
  deleteKey(alias: string): Promise<void>;
}
