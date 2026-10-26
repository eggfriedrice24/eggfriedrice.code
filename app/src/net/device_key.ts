// The device key: the P-256 key of the phone that efrd enrolled. The local Expo module
// app/modules/efr-device-key keeps it in the Secure Enclave and implements this
// interface; the connection only signs with it. The private key never leaves the
// module.

/** The key of this device. */
export interface DeviceKey {
  /**
   * The public key: the 65-byte uncompressed P-256 point (`04`, then X and Y), as
   * `device.enroll` sends it.
   */
  publicKey(): Promise<Uint8Array>;
  /**
   * Signs `message` with ECDSA P-256 and SHA-256 and returns the signature in DER
   * (X9.62), as `device.prove` sends it. It rejects when the key is gone, for example
   * after the app was installed again.
   */
  sign(message: Uint8Array): Promise<Uint8Array>;
}
