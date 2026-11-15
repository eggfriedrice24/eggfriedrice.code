import "../../global.css";

import Constants from "expo-constants";

import { appStorage } from "@/lib/native/kv";
import { AccessibilityInfo, Keyboard } from "react-native";

import { appPhase, followAppPhase, followAppState } from "@/lib/native/app-state";
import { systemClock } from "@/lib/native/clock";
import { newCommandId, openAppDatabase, openEventCaches } from "@/lib/native/database";
import { setHapticsEnabled } from "@/lib/native/haptics";
import { checkOwner, hasPasscode, ownerFacts } from "@/lib/native/owner-check";
import { followNetwork, followOnline } from "@/lib/native/netinfo";
import { animationFrames } from "@/lib/store/frame-store";
import { SqliteOutboxStorage } from "@/features/conversation/model/outbox-storage";
import { MachineMenu } from "@/features/machines/components/machine-menu";
import { AppProviders } from "@/features/navigation/components/app-providers";
import { LockGate, LockOverlay } from "@/features/navigation/components/lock-overlay";
import { RootStack } from "@/features/navigation/components/root-stack";
import { createAppServices } from "@/features/navigation/model/app-services";
import { linkGate } from "@/features/navigation/model/link-gate";
import type { PairingDeps } from "@/features/pairing/model/pair";
import { nativePairingDeps } from "@/features/pairing/native/native-pairing";

export { ErrorBoundary } from "expo-router";

// The root of the app (plan sections 2.2 and 2.5). The services start once, when the app
// loads: the runtime reads the paired machines and connects them, and the outbox reads
// the prompts of the last launch. The root stack gives each screen its header. With the Face
// ID unlock on, the stack mounts after the first unlock, and the lock screen and the cover
// lie over the whole window (lock-overlay.tsx).

/** A deep link to a conversation still has the blank root under it. */
export const unstable_settings = { initialRouteName: "index" };

/**
 * The pairing effects with the demo machines on them, in a dev or preview build. The test
 * is the one of lib/dev/dev-tools.ts, written here in full: Metro folds it in a build
 * without the dev tools and drops the require, so the demo server stays out of that
 * bundle.
 */
const withDevMachines: (pairing: PairingDeps | null) => PairingDeps | null =
  (typeof __DEV__ === "boolean" && __DEV__) || process.env["EXPO_PUBLIC_EFR_DEV_TOOLS"] === "1"
    ? (() => {
        const demo: typeof import("@/features/demo/native/demo-machines") = require("@/features/demo/native/demo-machines");
        return demo.withDevMachines;
      })()
    : (pairing) => pairing;

/** Reports an error that the app goes on after. Only a dev build shows it. */
function report(error: unknown): void {
  if (__DEV__) {
    console.warn(error);
  }
}

const services = createAppServices({
  // A dev or preview build sends the sockets of the demo machines to the demo server.
  pairing: withDevMachines(nativePairingDeps()),
  random: Math.random,
  schedule: animationFrames,
  clock: systemClock,
  now: Date.now,
  newId: newCommandId,
  openDatabase: openAppDatabase,
  openEventCaches,
  openOutboxStorage: (db) => SqliteOutboxStorage.open(db),
  keyValue: appStorage,
  queryStorage: appStorage,
  appVersion: Constants.expoConfig?.version ?? "0.0.0",
  lifecycle: { followAppState, followNetwork },
  followOnline,
  setHapticsEnabled,
  onError: report,
  lock: {
    followAppPhase,
    appPhase,
    checkOwner,
    ownerFacts,
    hasPasscode,
    dismissKeyboard: () => Keyboard.dismiss(),
    announce: (line) => AccessibilityInfo.announceForAccessibility(line),
  },
  linkGate,
});

function machineMenu(machine: string) {
  return <MachineMenu current={machine} />;
}

export default function RootLayout() {
  return (
    <AppProviders services={services}>
      <LockGate>
        <RootStack machineMenu={machineMenu} />
      </LockGate>
      <LockOverlay />
    </AppProviders>
  );
}
