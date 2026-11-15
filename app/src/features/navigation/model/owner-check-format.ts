// The words of the Face ID unlock (S-Lock, Settings round 9, S-ComposerR7 round 9): the
// name of the switch, the reason that iOS shows, the lines of the lock screen, of the row
// of Settings and of the mode menu. lib/lock/owner-check.ts has the types, which know no
// feature.

import type { CheckPurpose, FailedCheck, OwnerFacts, OwnerMethod } from "@/lib/lock/owner-check";

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
      return turnOnTitle(method);
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
