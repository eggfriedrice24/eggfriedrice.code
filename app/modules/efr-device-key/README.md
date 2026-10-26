# efr-device-key

A local Expo module for iOS. It makes and uses the device key of the efr app: a P-256
key in the Secure Enclave. efrd keeps the public key when you pair the phone
(`device.enroll`). On each connection the phone signs a challenge with the key, and efrd
checks the signature (`device.prove`).

## What the module does

- It makes the key with CryptoKit `SecureEnclave.P256.Signing.PrivateKey`. The access
  control is `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` with `.privateKeyUsage`. The
  key has no Face ID gate, because the app proves the key on each reconnect.
- The private key never leaves the Secure Enclave. The keychain keeps only the encrypted
  handle of the key (`dataRepresentation`), as a generic password of the service
  `dev.eggfriedrice.efr.device-key`, with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`.
  The handle is not in a backup and does not go to a new phone.
- It gives the public key as the 65-byte X9.63 point (`04`, then X and Y).
- It signs with ECDSA and SHA-256, and gives the signature in DER.
- The Simulator has no Secure Enclave. A Simulator build uses a software P-256 key in the
  keychain, so that you can test the pairing flow. A build for a device has no software
  path: `#if targetEnvironment(simulator)` removes it.

All values cross to JavaScript as standard base64 strings. The app does not call the
module directly. It uses `deviceKeyStore` in `src/features/pairing/model/device-key.ts`, which checks
the shape of each value that the module gives.

| Function               | Result                                                                     |
| ---------------------- | -------------------------------------------------------------------------- |
| `isHardwareBacked()`   | `true` for the Secure Enclave, `false` for the Simulator key               |
| `createKey(alias)`     | Makes a new key under `alias` in place of an old one; gives the public key |
| `publicKey(alias)`     | The public key under `alias`, or `null`                                    |
| `sign(alias, message)` | The DER signature of `message`                                             |
| `deleteKey(alias)`     | Removes the key; a missing key is not an error                             |

An alias is 1 to 64 ASCII letters, digits, `.`, `_` or `-`. The pairing flow makes a new
alias for each pairing (`device-<uuid>`), and removes the key of the old pairing only
after it keeps the new record. Thus a failed pairing does not break the old one.

## Why not expo-secure-enclave

We compared expo-secure-enclave 0.2.0 (npm, 2026-10-13). It makes a Secure Enclave P-256
key and signs in DER, but it does not fit:

- Each key has `.biometryCurrentSet`, so each signature needs Face ID or Touch ID. efr
  proves the key on each reconnect without a prompt (R3.7 of the research).
- It has no Simulator path: `isAvailable` is `false` in the Simulator.
- It is not in the package table of the research (R4), and it has Android code that efr
  does not use.

## Checks on an EAS build

The module cannot build on Linux, so CI does not compile it. Do these checks on the first
EAS development build on an iPhone:

1. The build log shows the pod `EfrDeviceKey` and no Swift error in
   `EfrDeviceKeyModule.swift`. The module uses the `@ExpoModule` and `@JS` macros of
   expo-modules-core 58.
2. In the app, `requireOptionalNativeModule("EfrDeviceKey")` is not `null`.
3. `isHardwareBacked()` is `true` on the iPhone and `false` in the Simulator.
4. Pair the phone with `efr device add`. The fingerprint that the phone shows is the same
   as the fingerprint at the terminal.
5. Close and open the app. The phone connects again without a new pairing, so the key
   loads from the keychain.
6. Lock the phone and keep the app open. A sign fails while the phone is locked and works
   again after you unlock it.
7. Delete the app and install it again. iOS can keep keychain items after a delete, so
   the phone can stay paired. Record what you see. Then revoke the device with
   `efr device revoke` and check that the phone must pair again.
