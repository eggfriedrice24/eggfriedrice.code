// What the pairing screens show (design 3.17 and the P-Errors board). `pairingProblem`
// maps a failure of the flow (./pair.ts) to the problem that the error screen shows, and
// `errorView` gives the words of that screen: the short word after "✗", the title, the
// reason and the one action. Pure, for the tests.
//
// The texts of the five drawn errors are the design's. The other errors (a bad code, a
// newer protocol, the camera, no scanner, the key, any other failure) use the same layout
// in the same voice (plan 5.2 item 8.19).

import { PROTOCOL_VERSION } from "@/lib/protocol/generated";
import type { PairingFailure } from "./pair";
import { machineName } from "./paired-device";

/** Why a pairing stopped, as the error screen shows it. */
export type PairingProblem =
  /** The code expired or was used (efrd refused the token, or the pairing ended so). */
  | { readonly kind: "expired" }
  /** A re-pair of `repairing` got the code of machine `code`. */
  | { readonly kind: "other_machine"; readonly code: string; readonly repairing: string }
  /** No connection came up: the phone is most likely off its tailnet. */
  | { readonly kind: "tailnet"; readonly machine: string }
  /** The user said no at the terminal. */
  | { readonly kind: "refused"; readonly machine: string }
  /**
   * efrd speaks another protocol. `direction` says which side is older; `unknown` when
   * efrd did not name its protocol.
   */
  | {
      readonly kind: "protocol";
      readonly direction: "efrd_old" | "app_old" | "unknown";
      readonly machine: string;
      readonly protocol: number | undefined;
      readonly version: string | undefined;
    }
  /** The code is for a newer version of the app. */
  | { readonly kind: "code_new" }
  /** The code is an efr code, but the app cannot read it. */
  | { readonly kind: "bad_code" }
  /** Another daemon answered at the address of the code. */
  | { readonly kind: "wrong_daemon"; readonly machine: string }
  /** The user did not let efr use the camera. */
  | { readonly kind: "camera" }
  /** This iPhone has no system code scanner. */
  | { readonly kind: "no_scanner" }
  /** The phone could not make or use its device key. */
  | { readonly kind: "key"; readonly message: string }
  /** Another failure, with the message of the flow. */
  | { readonly kind: "failed"; readonly message: string };

/** What the flow knows about the pairing when it fails. */
export interface ProblemContext {
  /** The short name of the machine of the code, such as `desktop`. */
  readonly machine: string;
  /** The name of the machine that the user pairs again, for a re-pair. */
  readonly repairing: string | undefined;
}

/**
 * The problem of `failure`, or null for a cancel (the screen then goes back to the
 * intro). efrd ends a pairing that expired before the answer with the same code as a no
 * at the terminal, so the message tells them apart.
 */
export function pairingProblem(
  failure: PairingFailure,
  context: ProblemContext,
): PairingProblem | null {
  switch (failure.kind) {
    case "cancelled":
      return null;
    case "not_efr":
    case "malformed":
      return { kind: "bad_code" };
    case "code_version":
      return { kind: "code_new" };
    case "other_machine":
      return {
        kind: "other_machine",
        code: machineName(failure),
        repairing: context.repairing ?? context.machine,
      };
    case "unreachable":
      return { kind: "tailnet", machine: context.machine };
    case "wrong_daemon":
      return { kind: "wrong_daemon", machine: context.machine };
    case "protocol_mismatch":
      return {
        kind: "protocol",
        direction: protocolDirection(failure.protocol),
        machine: context.machine,
        protocol: failure.protocol,
        version: failure.version,
      };
    case "token_refused":
      return { kind: "expired" };
    case "refused":
      return endedByTime(failure.message)
        ? { kind: "expired" }
        : { kind: "refused", machine: context.machine };
    case "key_failed":
      return { kind: "key", message: failure.message };
    default:
      return { kind: "failed", message: failure.message };
  }
}

/** Which side is older when efrd speaks protocol `protocol`. */
function protocolDirection(protocol: number | undefined): "efrd_old" | "app_old" | "unknown" {
  if (protocol === undefined) {
    return "unknown";
  }
  return protocol < PROTOCOL_VERSION ? "efrd_old" : "app_old";
}

/**
 * True when efrd ended the pairing because the code ran out: it expired before the
 * answer, or too many wrong tokens closed it (crates/efr-daemon/src/error.rs,
 * `PairingProblem::Ended`).
 */
function endedByTime(message: string): boolean {
  return /expired|too many/i.test(message);
}

/** A run of text in a title or a reason. */
export interface TextRun {
  readonly text: string;
  /**
   * `plain` is the text of the line; `mono` is a machine name or a version in SF Mono;
   * `command` is a command to run, in SF Mono and yolk, as in the shell.
   */
  readonly look: "plain" | "mono" | "command";
}

/** The one action of an error screen. */
export type ErrorAction = "scan_again" | "open_tailscale" | "try_again" | "done" | "open_settings";

/** The label of each action. */
export const ERROR_ACTION_LABEL = {
  scan_again: "Scan again",
  open_tailscale: "Open Tailscale",
  try_again: "Try again",
  done: "Done",
  open_settings: "Open Settings",
} as const satisfies Record<ErrorAction, string>;

/** The words of one error screen. */
export interface ErrorView {
  /** The short word after "✗", such as "code expired". */
  readonly word: string;
  readonly title: readonly TextRun[];
  readonly reason: readonly TextRun[];
  readonly action: ErrorAction;
}

/** The command that makes a pairing code. */
export const PAIR_COMMAND = "efr device add";

function plain(text: string): TextRun {
  return { text, look: "plain" };
}

function mono(text: string): TextRun {
  return { text, look: "mono" };
}

const COMMAND: TextRun = { text: PAIR_COMMAND, look: "command" };

/** The words of the error screen of `problem`. */
export function errorView(problem: PairingProblem): ErrorView {
  switch (problem.kind) {
    case "expired":
      return {
        word: "code expired",
        title: [plain("This code has expired")],
        reason: [
          plain("Codes from "),
          COMMAND,
          plain(" only work for a short time. Run it again and scan the new code."),
        ],
        action: "scan_again",
      };
    case "other_machine":
      return {
        word: "wrong machine",
        title: [plain("This code is from "), mono(problem.code)],
        reason: [
          plain(
            `You're re-pairing ${problem.repairing}, but this code came from ${problem.code}. ` +
              `Run the command on ${problem.repairing} and scan that code.`,
          ),
        ],
        action: "scan_again",
      };
    case "tailnet":
      return {
        word: "tailscale not connected",
        title: [plain("Tailscale is not connected")],
        reason: [
          plain(
            `This iPhone isn't on your tailnet, so it can't reach ${problem.machine}. ` +
              "Turn Tailscale on, then come back and try again.",
          ),
        ],
        action: "open_tailscale",
      };
    case "refused":
      return {
        word: "refused",
        title: [plain("Refused on "), mono(problem.machine)],
        reason: [
          plain(`Someone answered no in the terminal on ${problem.machine}. Nothing was paired.`),
        ],
        action: "try_again",
      };
    case "protocol":
      return protocolView(problem);
    case "code_new":
      return {
        word: "app too old",
        title: [plain("This code is for a newer app")],
        reason: [plain("Update the app, then scan the code again.")],
        action: "done",
      };
    case "bad_code":
      return {
        word: "bad code",
        title: [plain("This code does not work")],
        reason: [
          plain("The app cannot read this pairing code. Run "),
          COMMAND,
          plain(" again and scan the new code."),
        ],
        action: "scan_again",
      };
    case "wrong_daemon":
      return {
        word: "wrong machine",
        title: [plain("Another machine answered")],
        reason: [
          plain(`The computer at the address of this code is not ${problem.machine}. Run `),
          COMMAND,
          plain(` on ${problem.machine} again and scan the new code.`),
        ],
        action: "scan_again",
      };
    case "camera":
      return {
        word: "camera off",
        title: [plain("efr cannot use the camera")],
        reason: [plain("Turn on the camera for efr in Settings, then scan the code again.")],
        action: "open_settings",
      };
    case "no_scanner":
      return {
        word: "no scanner",
        title: [plain("This iPhone cannot scan the code")],
        reason: [plain("The code scanner needs iOS 16 or later and a camera.")],
        action: "done",
      };
    case "key":
      return {
        word: "no device key",
        title: [plain("The phone could not make its key")],
        reason: [plain(sentence(problem.message))],
        action: "try_again",
      };
    default:
      return {
        word: "pairing failed",
        title: [plain("The pairing did not finish")],
        reason: [plain(sentence(problem.message))],
        action: "try_again",
      };
  }
}

/** The words of a protocol mismatch, with the facts that efrd named. */
function protocolView(problem: Extract<PairingProblem, { kind: "protocol" }>): ErrorView {
  const { machine, protocol, version } = problem;
  const runs = (needs: string): TextRun[] => {
    if (protocol === undefined) {
      return [plain(`${machine} speaks another protocol. ${needs}`)];
    }
    if (version === undefined) {
      return [plain(`${machine} speaks protocol ${protocol}. ${needs}`)];
    }
    return [
      plain(`${machine} runs efrd `),
      mono(version),
      plain(`, which speaks protocol ${protocol}. ${needs}`),
    ];
  };
  switch (problem.direction) {
    case "app_old":
      return {
        word: "app too old",
        title: [plain("This app is too old for "), mono(machine)],
        reason: runs(
          `This app speaks protocol ${PROTOCOL_VERSION}. Update the app, then pair again.`,
        ),
        action: "done",
      };
    case "efrd_old":
      return {
        word: "efrd too old",
        title: [plain("efr on "), mono(machine), plain(" is too old")],
        reason: runs(
          `This app needs protocol ${PROTOCOL_VERSION}. Update efr on ${machine}, then pair again.`,
        ),
        action: "done",
      };
    default:
      return {
        word: "other protocol",
        title: [plain("efr on "), mono(machine), plain(" speaks another protocol")],
        reason: runs(
          `This app needs protocol ${PROTOCOL_VERSION}. ` +
            `Update efr on ${machine} or the app, then pair again.`,
        ),
        action: "done",
      };
  }
}

/** `message` as a sentence: a capital first letter and a full stop. */
function sentence(message: string): string {
  const trimmed = message.trim();
  if (trimmed === "") {
    return "The app does not know why.";
  }
  const first = trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return /[.!?]$/.test(first) ? first : `${first}.`;
}

/** The text of `runs`, for VoiceOver. */
export function plainText(runs: readonly TextRun[]): string {
  return runs.map((run) => run.text).join("");
}

/** The title of the nav row: "Pair a machine", or "Re-pair laptop" for a re-pair. */
export function pairingTitle(repairing: string | undefined): string {
  return repairing === undefined ? "Pair a machine" : `Re-pair ${repairing}`;
}

/** The words of the success screen. */
export interface SuccessWords {
  /** "✓ paired", or "✓ re-paired" for a re-pair. */
  readonly status: string;
  /** The hint above Start. */
  readonly hint: string;
}

/** The words of the success screen for `machine`. */
export function successWords(machine: string, repaired: boolean): SuccessWords {
  return repaired
    ? { status: "✓ re-paired", hint: "goes back to where you were" }
    : { status: "✓ paired", hint: `opens a blank conversation on ${machine}` };
}

/** The status line under the machine on success, such as "efrd 0.0.5 · protocol 1". */
export function daemonLine(status: {
  readonly version: string;
  readonly protocol: number;
}): string {
  return `efrd ${status.version} · protocol ${status.protocol}`;
}
