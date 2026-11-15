// The check of the iPhone's owner on iOS (LocalAuthentication, the policy
// deviceOwnerAuthentication): Face ID or Touch ID, and the passcode after them, so an
// iPhone without biometrics checks with its passcode. model/owner-check.ts says what the
// composer does with the answer.

import {
  AuthenticationType,
  authenticateAsync,
  getEnrolledLevelAsync,
  SecurityLevel,
  supportedAuthenticationTypesAsync,
} from "expo-local-authentication";

import { CHECK_REASON, type OwnerCheck, type OwnerMethod } from "../model/owner-check";

/** True when the iPhone has a passcode, so it can check its owner. */
export async function hasPasscode(): Promise<boolean> {
  return (await getEnrolledLevelAsync()) !== SecurityLevel.NONE;
}

/** The way the iPhone checks first. */
async function methodOf(): Promise<OwnerMethod> {
  const types = await supportedAuthenticationTypesAsync();
  if (types.includes(AuthenticationType.FACIAL_RECOGNITION)) {
    return "Face ID";
  }
  if (types.includes(AuthenticationType.FINGERPRINT)) {
    return "Touch ID";
  }
  return "the passcode";
}

/** Asks iOS to check that the owner holds the iPhone. */
export async function checkOwner(): Promise<OwnerCheck> {
  if (!(await hasPasscode())) {
    return { kind: "no_passcode" };
  }
  const method = await methodOf();
  const result = await authenticateAsync({
    promptMessage: CHECK_REASON,
    disableDeviceFallback: false,
  });
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
    case "not_enrolled":
    case "not_available":
      return { kind: "no_passcode" };
    default:
      return { kind: "failed", method };
  }
}
