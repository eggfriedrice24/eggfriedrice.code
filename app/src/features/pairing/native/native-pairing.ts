// The real effects of the pairing flow on an iPhone: the device key module, expo-camera,
// expo-secure-store, expo-crypto, and the WebSocket and the clock of src/lib/native. The
// tests use fakes for each, so this file holds no logic of its own.

import Constants from "expo-constants";
import { Camera, CameraView } from "expo-camera";
import * as Crypto from "expo-crypto";
import * as SecureStore from "expo-secure-store";

import { systemClock } from "@/lib/native/clock";
import { nativeSockets } from "@/lib/native/socket";
import { EfrDeviceKey } from "../../../../modules/efr-device-key";
import { deviceKeyStore } from "../model/device-key";
import type { DeviceKeyStore } from "../model/device-key";
import { deviceName } from "../model/device-name";
import type { PairingDeps } from "../model/pair";
import { secureStorePairedDevices } from "../model/paired-device";
import type { PairedDeviceStore } from "../model/paired-device";
import type { ScannerApi } from "../model/scanner";

/** The client program that `hello` names. */
export const CLIENT = `efr-app ${Constants.expoConfig?.version ?? "0.0.0"}`;

/** The key store of the native module, or null when the build has no module (Expo Go). */
export function nativeDeviceKeys(): DeviceKeyStore | null {
  return EfrDeviceKey === null ? null : deviceKeyStore(EfrDeviceKey);
}

/** The pairing record in expo-secure-store. */
export function nativePairedDevices(): PairedDeviceStore {
  return secureStorePairedDevices(SecureStore, SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY);
}

/** The system QR scanner of expo-camera. */
export function nativeScanner(): ScannerApi {
  return {
    available: CameraView.isModernBarcodeScannerAvailable,
    async requestPermission() {
      const response = await Camera.requestCameraPermissionsAsync();
      return response.granted;
    },
    launch: () => CameraView.launchScanner({ barcodeTypes: ["qr"], isHighlightingEnabled: true }),
    dismiss: () => CameraView.dismissScanner(),
    onScanned(listener) {
      return CameraView.onModernBarcodeScanned((event) => listener(event.data));
    },
  };
}

/** The name that the phone suggests for itself, such as "Anna's iPhone". */
export function suggestedDeviceName(): string {
  return deviceName(Constants.deviceName);
}

/** The effects of the pairing flow, or null when the build has no device key module. */
export function nativePairingDeps(): PairingDeps | null {
  const keys = nativeDeviceKeys();
  if (keys === null) {
    return null;
  }
  return {
    sockets: nativeSockets,
    clock: systemClock,
    keys,
    devices: nativePairedDevices(),
    sha256: async (data) =>
      new Uint8Array(await Crypto.digest(Crypto.CryptoDigestAlgorithm.SHA256, data)),
    newKeyAlias: () => `device-${Crypto.randomUUID()}`,
    now: () => new Date(),
    client: CLIENT,
  };
}
