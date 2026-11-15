// The check of the iPhone's owner before a message runs in auto (S-ComposerR7 5, AU-Auto
// 4-6): Face ID or Touch ID, then the passcode of the iPhone. The prompt is the one of
// iOS; the app draws only the composer around it. The check runs at the tap on Send,
// never when the outbox sends the message later, and a message that did not pass stays
// in the field with one line that says why.
//
// It is a gate of the app: efrd cannot verify it (ADR 0022, item 7). efrd still runs a
// phone prompt in auto only when the prompt names auto.
//
// Pure TypeScript: the native adapter is native/owner-check.ts.

/** How the iPhone checks its owner: the words of the not-sent line. */
export type OwnerMethod = "Face ID" | "Touch ID" | "the passcode";

/** The end of one check. */
export type OwnerCheck =
  | { readonly kind: "passed" }
  | { readonly kind: "failed"; readonly method: OwnerMethod }
  | { readonly kind: "cancelled"; readonly method: OwnerMethod }
  /** iOS locked biometrics after many tries. */
  | { readonly kind: "locked"; readonly method: OwnerMethod }
  /** The iPhone has no passcode, so it cannot check its owner. */
  | { readonly kind: "no_passcode" };

/** A check of the owner, as the composer gets it. */
export type CheckOwner = () => Promise<OwnerCheck>;

/** The reason that iOS shows with the passcode screen. */
export const CHECK_REASON = "Send this message in auto";

/** The one line of a message that did not go: "Not sent." then why, then what to do. */
export interface NotSentLine {
  /** "Not sent.", in text_primary. */
  readonly head: string;
  /** Why and how to try again, in text_muted. */
  readonly rest: string;
}

/** The line of a check that did not pass, or none for a pass. */
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

function capital(words: string): string {
  return words.charAt(0).toUpperCase() + words.slice(1);
}
