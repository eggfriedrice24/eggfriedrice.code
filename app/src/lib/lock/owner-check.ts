// The check of the iPhone's owner and its words (S-Lock, Settings round 9, S-ComposerR7
// round 9): Face ID or Touch ID, then the passcode of the iPhone. The prompt is the one of
// iOS. The Face ID unlock of the app (features/navigation/model/app-lock.ts) runs it when
// the app opens and when the switch of Settings turns on or off.
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

/** A check of the owner. `reason` is the line that iOS shows with the passcode screen. */
export type CheckOwner = (reason: string) => Promise<OwnerCheck>;

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

function capital(words: string): string {
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** The name of the switch (D6): "Face ID unlock", "Touch ID unlock" or "Passcode unlock". */
export function unlockName(method: OwnerMethod): string {
  return method === "the passcode" ? "Passcode unlock" : `${method} unlock`;
}

/** The reason of iOS: "Unlock efr", "Turn on Face ID unlock", "Turn off Face ID unlock". */
export function checkReason(purpose: CheckPurpose, method: OwnerMethod): string {
  switch (purpose) {
    case "unlock":
      return "Unlock efr";
    case "turn_on":
      return `Turn on ${unlockName(method)}`;
    default:
      return `Turn off ${unlockName(method)}`;
  }
}

/** The line of the lock screen after a check that did not pass (S-Lock 8, 9). */
export function lockLine(check: FailedCheck): string {
  switch (check.kind) {
    case "failed":
      return `${capital(check.method)} did not pass.`;
    case "cancelled":
      return `You cancelled ${check.method}.`;
    default:
      return `${capital(check.method)} is locked. Unlock asks for the passcode.`;
  }
}

/** The VoiceOver hint of Unlock after `failed`, or before any failure. */
export function unlockHint(method: OwnerMethod, failed: FailedCheck | null): string {
  if (failed?.kind === "locked" || method === "the passcode") {
    return "Asks for the passcode of this iPhone.";
  }
  return `Asks for ${method}.`;
}

/** The line under the row of Settings while the iPhone has no passcode. */
export const NO_PASSCODE_ROW_LINE = "Needs a passcode on this iPhone.";

/** The line under the row of Settings: "Opens efr with Face ID. Auto from this phone needs it." */
export function unlockRowLine(facts: OwnerFacts): string {
  if (!facts.passcode) {
    return NO_PASSCODE_ROW_LINE;
  }
  return `Opens efr with ${facts.method}. Auto from this phone needs it.`;
}

/** The line of the row after efr turned the unlock off (D5, Settings r9 6). */
export function noPasscodeNotice(method: OwnerMethod): string {
  return `${unlockName(method)} is off: this iPhone has no passcode.`;
}

/** The line of Auto in the mode menu while the unlock is off: "Needs Face ID unlock in Settings." */
export function autoNeedsLine(method: OwnerMethod): string {
  return `Needs ${unlockName(method)} in Settings.`;
}

/** The third item of the mode menu while the unlock is off: "Turn on Face ID unlock". */
export function turnOnTitle(method: OwnerMethod): string {
  return `Turn on ${unlockName(method)}`;
}

/** The reason of the per-message check of round 7; it goes with that check. */
export const CHECK_REASON = "Send this message in auto";

/** The one line of a message that did not go (round 7; it goes with that check). */
export interface NotSentLine {
  readonly head: string;
  readonly rest: string;
}

/** The line of a check that did not pass, or none for a pass (round 7). */
export function notSentLine(check: OwnerCheck): NotSentLine | undefined {
  const head = "Not sent.";
  switch (check.kind) {
    case "passed":
      return undefined;
    case "failed":
      return { head, rest: `${capital(check.method)} did not pass. Tap Send to try again.` };
    case "cancelled":
      return { head, rest: `You cancelled ${check.method}. Tap Send to try again.` };
    case "locked":
      return {
        head,
        rest: `${capital(check.method)} is locked. Unlock the iPhone, then tap Send.`,
      };
    default:
      return { head, rest: "Auto needs a passcode on this iPhone. Set one, or send in cautious." };
  }
}
