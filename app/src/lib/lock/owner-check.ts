// The check of the iPhone's owner (S-Lock, Settings round 9, S-ComposerR7 round 9): Face
// ID or Touch ID, then the passcode of the iPhone. The prompt is the one of iOS. The Face
// ID unlock of the app (features/navigation/model/app-lock.ts) runs it when the app opens
// and when the switch of Settings turns on or off. Its words are in
// features/navigation/model/owner-check-format.ts.
//
// It is a gate of the app: efrd cannot verify it (ADR 0022, item 7). efrd still runs a
// phone prompt in auto only when the prompt names auto.
//
// Pure TypeScript: the native adapter is lib/native/owner-check.ts.

/** How the iPhone checks its owner first. */
export type OwnerMethod = "Face ID" | "Touch ID" | "the passcode";

/** The end of one check. */
export type OwnerCheck =
  | { readonly kind: "passed" }
  | { readonly kind: "failed"; readonly method: OwnerMethod }
  | { readonly kind: "cancelled"; readonly method: OwnerMethod }
  /** iOS locked biometrics after many tries; the next check asks for the passcode. */
  | { readonly kind: "locked"; readonly method: OwnerMethod }
  /** The iPhone has no passcode, so it cannot check its owner. */
  | { readonly kind: "no_passcode" };

/** A check that did not pass and that leaves the lock as it is. */
export type FailedCheck = Exclude<OwnerCheck, { kind: "passed" } | { kind: "no_passcode" }>;

/**
 * A check of the owner. `reason` is the line that iOS shows with the passcode screen.
 * `method` is what the caller read in the facts just before, so a check reads them no
 * second time; a check that did not pass names it.
 */
export type CheckOwner = (reason: string, method: OwnerMethod) => Promise<OwnerCheck>;

/** What the iPhone can check now. */
export interface OwnerFacts {
  readonly passcode: boolean;
  /** The way that iOS checks first: "the passcode" when no biometric is set up. */
  readonly method: OwnerMethod;
}

/** Why a check runs. */
export type CheckPurpose = "unlock" | "turn_on" | "turn_off";

/** The facts before iOS answers: Face ID, with a passcode. */
export const DEFAULT_FACTS: OwnerFacts = { passcode: true, method: "Face ID" };
