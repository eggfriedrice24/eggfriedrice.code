// The check of the iPhone's owner on iOS (LocalAuthentication, the policy
// deviceOwnerAuthentication): Face ID or Touch ID, and the passcode after them, so an
// iPhone without biometrics checks with its passcode. lib/lock/owner-check.ts has the
// types and the words.

import {
  AuthenticationType,
  authenticateAsync,
  getEnrolledLevelAsync,
  SecurityLevel,
  supportedAuthenticationTypesAsync,
} from "expo-local-authentication";

import type { OwnerCheck, OwnerFacts, OwnerMethod } from "@/lib/lock/owner-check";

/** True when the iPhone has a passcode, so it can check its owner. */
export async function hasPasscode(): Promise<boolean> {
  return (await getEnrolledLevelAsync()) !== SecurityLevel.NONE;
}

/**
 * What the iPhone can check now. The name comes from what is set up (the enrolled
 * level), not from the hardware: an iPhone whose Face ID is not set up checks with the
 * passcode.
 */
export async function ownerFacts(): Promise<OwnerFacts> {
  const level = await getEnrolledLevelAsync();
  if (level === SecurityLevel.NONE) {
    return { passcode: false, method: "the passcode" };
  }
  if (level === SecurityLevel.SECRET) {
    return { passcode: true, method: "the passcode" };
  }
  const types = await supportedAuthenticationTypesAsync();
  let method: OwnerMethod = "the passcode";
  if (types.includes(AuthenticationType.FACIAL_RECOGNITION)) {
    method = "Face ID";
  } else if (types.includes(AuthenticationType.FINGERPRINT)) {
    method = "Touch ID";
  }
  return { passcode: true, method };
}

/**
 * Asks iOS to check that the owner holds the iPhone. `reason` shows with the passcode
 * screen. Only a missing passcode gives `no_passcode`: a Face ID that is not set up, or
 * that efr may not use, still falls back to the passcode.
 */
export async function checkOwner(reason: string): Promise<OwnerCheck> {
  const method = (await ownerFacts()).method;
  const result = await authenticateAsync({ promptMessage: reason, disableDeviceFallback: false });
  if (result.success) {
    return { kind: "passed" };
  }
  switch (result.error) {
    case "user_cancel":
    case "system_cancel":
    case "app_cancel":
    case "user_fallback":
      return { kind: "cancelled", method };
    case "lockout":
      return { kind: "locked", method };
    case "passcode_not_set":
      return { kind: "no_passcode" };
    default:
      return { kind: "failed", method };
  }
}
