// The device key: the P-256 key of the phone that efrd enrolled. The local Expo module
// app/modules/efr-device-key keeps it in the Secure Enclave, and src/features/pairing/model/device-key.ts
// gives it to the app; the connection only signs with it. The private key never leaves
// the module.

/** The key of this device, as the connection uses it. */
export interface DeviceKey {
  /**
   * Signs `message` with ECDSA P-256 and SHA-256 and returns the signature in DER
   * (X9.62), as `device.prove` sends it. It rejects when the key is gone, for example
   * after the app was installed again.
   */
  sign(message: Uint8Array): Promise<Uint8Array>;
}
