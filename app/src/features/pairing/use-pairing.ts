// The hooks of the pairing screens. The pairing session (model/pairing-session.ts) is one
// for the app: the no-machine root starts a scan, and the pairing modal shows the rest of
// it. This file makes the session with the real effects, and gives the screens its state
// and the ways out of the modal.

import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { useNavigation, useRouter } from "expo-router";
import type { NavigationProp, ParamListBase } from "expo-router/react-navigation";
import { createContext, use } from "react";
import { AccessibilityInfo, Linking } from "react-native";

import {
  machineStatusQueryOptions,
  pairMachineMutationOptions,
} from "@/features/machines/machine-queries";
import type { MachineStatus } from "@/features/machines/model/machine-connection";
import type { MachineId } from "@/features/machines/model/machine-ref";
import { findMachine } from "@/features/machines/model/machine-registry";
import type { Runtime } from "@/features/machines/model/runtime";
import type { PreferencesStore } from "@/features/settings/model/preferences";
import { usePreferences, useRuntime } from "@/features/navigation/use-app-services";
import { followAppState } from "@/lib/native/app-state";
import { haptic } from "@/lib/native/haptics";
import { useReduceMotion } from "@/lib/native/reduce-motion";
import {
  createPairingSession,
  phaseFor,
  type PairingPhase,
  type PairingSession,
  type RepairTarget,
} from "./model/pairing-session";
import { nativeScanner, suggestedDeviceName } from "./native/native-pairing";

/**
 * The Tailscale app. NOTE: the URL scheme is not verified on a device yet (design note
 * 14, plan section 6 item 11). When no app opens it, the App Store page opens.
 */
export const TAILSCALE_URL = "tailscale://";

/** The App Store page of Tailscale for iOS. */
export const TAILSCALE_STORE_URL = "https://apps.apple.com/app/tailscale/id1470499037";

/**
 * A pairing session that replaces the one of the app, for the tests and the demo
 * machine. Without it, the screens use the session of the runtime.
 */
export const PairingSessionContext = createContext<PairingSession | null>(null);

/** One session for each runtime: the runtime lives as long as the app. */
const sessions = new WeakMap<Runtime, PairingSession>();

/** The session of `runtime`, made the first time. */
function sessionOf(
  runtime: Runtime,
  client: QueryClient,
  preferences: PreferencesStore,
): PairingSession {
  let session = sessions.get(runtime);
  if (session === undefined) {
    session = createPairingSession({
      client,
      mutation: pairMachineMutationOptions(runtime),
      registry: runtime.registry,
      scanner: nativeScanner(),
      deviceName: suggestedDeviceName,
      haptic,
      announce: (text) => AccessibilityInfo.announceForAccessibility(text),
      followAppState,
      openTailscale,
      openSettings: () => {
        void Linking.openSettings().catch(() => undefined);
      },
      selectMachine: (machine) => preferences.setLastMachine(machine),
    });
    sessions.set(runtime, session);
  }
  return session;
}

/** Opens Tailscale, or its App Store page when it is not installed. */
function openTailscale(): void {
  Linking.openURL(TAILSCALE_URL)
    .catch(() => Linking.openURL(TAILSCALE_STORE_URL))
    .catch(() => undefined);
}

/** The pairing session of the app (or the one of `PairingSessionContext`). */
export function usePairingSession(): PairingSession {
  const injected = use(PairingSessionContext);
  const runtime = useRuntime();
  const client = useQueryClient();
  const preferences = usePreferences();
  return injected ?? sessionOf(runtime, client, preferences);
}

/**
 * The machine that `machine` names, as the target of a re-pair, or null for a new
 * machine. An id that is not paired (any more) pairs a new machine.
 */
export function useRepairTarget(
  session: PairingSession,
  machine: MachineId | null,
): RepairTarget | null {
  const found = useSelector(session.registry, (state) =>
    machine === null ? undefined : findMachine(state, machine),
  );
  return found === undefined ? null : { id: found.id, name: found.name };
}

/** The phase that the screen of target `repair` shows. */
export function usePairingPhase(session: PairingSession, repair: MachineId | null): PairingPhase {
  return useSelector(session.state, (state) => phaseFor(state, repair));
}

/** What efrd on a newly paired machine said about itself, once it is connected. */
export function usePairedStatus(machine: MachineId | null): MachineStatus | undefined {
  const runtime = useRuntime();
  const query = useQuery({
    ...machineStatusQueryOptions(runtime, machine ?? ""),
    enabled: machine !== null,
  });
  return query.data;
}

/** The ways out of the pairing modal. */
export interface PairingExits {
  /** Closes the modal: the screen under it shows again as it was. */
  leave(): void;
  /**
   * Closes the modal and every screen above the root, so a blank root shows (design
   * note 13: pairing success and Start). The root keeps its key, so it does not mount
   * again. The modal goes with a short cross-fade (design section 1), and with none
   * when Reduce Motion is on.
   */
  toRoot(): void;
}

/** The ways out of the pairing modal. */
export function usePairingExits(): PairingExits {
  const router = useRouter();
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const reduceMotion = useReduceMotion();
  return {
    leave: () => {
      if (router.canDismiss()) {
        router.dismiss();
      } else {
        router.replace("/");
      }
    },
    toRoot: () => {
      const state = navigation.getState();
      const root = state?.routes[0];
      if (state === undefined || root === undefined) {
        router.replace("/");
        return;
      }
      // The options of the modal also animate its removal, as Settings does for Forget.
      navigation.setOptions({ animation: reduceMotion ? "none" : "fade" });
      // NOTE: expo-router 58 takes only a full state for a reset (plan W0 fact S3).
      navigation.reset({ ...state, index: 0, routes: [root], stale: false });
    },
  };
}
