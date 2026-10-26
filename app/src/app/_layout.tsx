import "../../global.css";

import Constants from "expo-constants";

import { appStorage } from "@/lib/native/kv";
import { followAppState } from "@/lib/native/app-state";
import { systemClock } from "@/lib/native/clock";
import { newCommandId, openAppDatabase, openEventCaches } from "@/lib/native/database";
import { setHapticsEnabled } from "@/lib/native/haptics";
import { followNetwork, followOnline } from "@/lib/native/netinfo";
import { animationFrames } from "@/lib/store/frame-store";
import { SqliteOutboxStorage } from "@/features/conversation/model/outbox-storage";
import { MachineMenu } from "@/features/machines/components/machine-menu";
import { AppProviders } from "@/features/navigation/components/app-providers";
import { RootStack } from "@/features/navigation/components/root-stack";
import { createAppServices } from "@/features/navigation/model/app-services";
import { nativePairingDeps } from "@/features/pairing/native/native-pairing";

export { ErrorBoundary } from "expo-router";

// The root of the app (plan sections 2.2 and 2.5). The services start once, when the app
// loads: the runtime reads the paired machines and connects them, and the outbox reads
// the prompts of the last launch. The root stack gives each screen its header.

/** A deep link to a conversation still has the blank root under it. */
export const unstable_settings = { initialRouteName: "index" };

/** Reports an error that the app goes on after. Only a dev build shows it. */
function report(error: unknown): void {
  if (__DEV__) {
    console.warn(error);
  }
}

const services = createAppServices({
  pairing: nativePairingDeps(),
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
});

function machineMenu(machine: string) {
  return <MachineMenu current={machine} />;
}

export default function RootLayout() {
  return (
    <AppProviders services={services}>
      <RootStack machineMenu={machineMenu} />
    </AppProviders>
  );
}
