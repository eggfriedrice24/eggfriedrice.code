// The real effects of the pairing flow on an iPhone: the device key module, expo-camera,
// expo-secure-store, expo-crypto and the WebSocket of React Native. The tests use fakes
// for each, so this file holds no logic of its own.

import Constants from "expo-constants";
import { Camera, CameraView } from "expo-camera";
import * as Crypto from "expo-crypto";
import * as SecureStore from "expo-secure-store";

import { EfrDeviceKey } from "../../modules/efr-device-key";
import type { OpenSocket, Schedule } from "./connection";
import { deviceKeyStore } from "./device_key";
import type { DeviceKeyStore } from "./device_key";
import { deviceName } from "./device_name";
import type { PairingDeps } from "./pair";
import { secureStorePairedDevices } from "./paired_device";
import type { PairedDeviceStore } from "./paired_device";
import type { ScannerApi } from "./scanner";

/** The client program that `hello` names. */
const CLIENT = `efr-app ${Constants.expoConfig?.version ?? "0.0.0"}`;

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

/** A WebSocket of React Native. It sets no Origin header of its own. */
export const openNativeSocket: OpenSocket = (url, handlers) => {
  const socket = new WebSocket(url);
  let closed = false;
  const close = (code: number, reason: string): void => {
    if (!closed) {
      closed = true;
      handlers.close(code, reason);
    }
  };
  socket.addEventListener("open", () => handlers.open());
  socket.addEventListener("message", (event) => handlers.message(event.data));
  // React Native sends an error and then a close for a failed socket; the first one counts.
  socket.addEventListener("error", () => close(1006, ""));
  socket.addEventListener("close", (event) => close(event.code ?? 1006, event.reason ?? ""));
  return {
    send: (text) => socket.send(text),
    close: () => socket.close(),
  };
};

/** Timers of the JS runtime. */
export const nativeSchedule: Schedule = (ms, run) => {
  const timer = setTimeout(run, ms);
  return () => clearTimeout(timer);
};

/** The effects of the pairing flow, or null when the build has no device key module. */
export function nativePairingDeps(): PairingDeps | null {
  const keys = nativeDeviceKeys();
  if (keys === null) {
    return null;
  }
  return {
    openSocket: openNativeSocket,
    schedule: nativeSchedule,
    keys,
    devices: nativePairedDevices(),
    sha256: async (data) =>
      new Uint8Array(await Crypto.digest(Crypto.CryptoDigestAlgorithm.SHA256, data)),
    newKeyAlias: () => `device-${Crypto.randomUUID()}`,
    now: () => new Date(),
    client: CLIENT,
  };
}
