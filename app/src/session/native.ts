// The session of the app on an iPhone: the real effects for src/session/phone_session.ts.
// This file holds no logic of its own; the tests use fakes for each effect.
//
// The client hears of the foreground from AppState (src/net/native/app_state.ts). No
// package of the app reports a change of the network path, such as from Wi-Fi to
// cellular: the liveness rule and the backoff of the client find a dead socket instead.

import { followAppState } from "@/lib/native/app-state";
import { systemClock } from "@/lib/native/clock";
import { nativeSockets } from "@/lib/native/socket";
import { CLIENT, nativePairingDeps, nativeScanner, suggestedDeviceName } from "@/pairing/native";
import { newCommandId, openEventCache } from "@/lib/native/database";
import { createPhoneSession, type LivePhoneSession } from "./phone_session";

/** A new session with the real effects. Call `start` to read the pairing and connect. */
export function createNativeSession(): LivePhoneSession {
  return createPhoneSession({
    pairing: nativePairingDeps(),
    scanner: nativeScanner(),
    deviceName: suggestedDeviceName,
    sockets: nativeSockets,
    clock: systemClock,
    random: Math.random,
    client: CLIENT,
    openCache: openEventCache,
    newCommandId,
    follow: followAppState,
  });
}
