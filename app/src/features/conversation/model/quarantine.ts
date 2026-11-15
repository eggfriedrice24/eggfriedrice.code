// The quarantine question of the phone (Q-Quarantine 1 to 6): with the reviewer off, a
// call in auto changed git settings that run programs, efr moved them to quarantine and
// asks whether to keep them. Pure, so each state has a test.
//
// - The strip is one row above the status line: the yolk dot and the question, `keep
//   .git/config (core.fsmonitor)?` or `keep 4 git changes?`.
// - The sheet lists only the changes that went to quarantine (a file left in place cannot
//   be kept): the path, and what it does in efr's words. One quiet line follows.
// - efrd takes the answer only from the terminal: a phone may not take a git setting out
//   of quarantine (efr-conversation refuses it with `RemoteSurfaceAnswer`). So the strip
//   and the sheet say "answer on <machine>" in place of Keep and Leave, as for an approval
//   that only the terminal can answer, and the status line says "waiting in the
//   terminal".
// - After the answer: "kept", "left in quarantine", or "not answered · left in
//   quarantine" when the question expired or the turn stopped. An answer of the terminal
//   while the sheet shows adds where it came from: "left in quarantine · on desktop".

import type { Origin } from "@/lib/protocol/generated";
import type { QuarantineQuestionView } from "@/lib/stream/reducer";
import { placeOf, surfaceWhat } from "./exit-words";

/** The quiet line of the sheet. */
export const QUARANTINE_QUIET =
  "Git settings that run programs, moved to quarantine, where nothing reads them.";

/** The strip of the question that waits first. */
export interface QuarantineStripModel {
  readonly callId: string;
  /** `keep .git/config (core.fsmonitor)?` or `keep 4 git changes?`. */
  readonly line: string;
  /** The VoiceOver label of the strip. */
  readonly label: string;
  /** `answer on desktop`, in place of the buttons. */
  readonly terminal: string;
}

/** One change of the sheet. */
export interface QuarantineRow {
  /** The path, relative to the working directory of the turn when it lies in it. */
  readonly path: string;
  /** What it does, such as `core.fsmonitor` or `a new hook`. */
  readonly what: string;
}

/** The sheet of a question. */
export interface QuarantineSheetModel {
  readonly rows: readonly QuarantineRow[];
  readonly quiet: string;
  /** `answer on desktop` while the question waits. */
  readonly terminal: string | undefined;
  /** How the question ended, such as `left in quarantine · on desktop`; dims the rows. */
  readonly closed: string | undefined;
}

/** The terminals of efr: an answer from one of them names the machine. */
const TERMINAL_ORIGINS: ReadonlySet<Origin> = new Set(["shell", "cli", "proxy"]);

function rowsOf(question: QuarantineQuestionView, cwd: string | undefined): QuarantineRow[] {
  return question.changes
    .filter((change) => change.quarantined)
    .map((change) => ({
      path: placeOf(change.path, cwd),
      what: surfaceWhat(change).replace(/\s+/g, " "),
    }));
}

/** The question of the strip: one change by its path and what it does, else the count. */
export function quarantineLine(question: QuarantineQuestionView, cwd: string | undefined): string {
  const rows = rowsOf(question, cwd);
  const first = rows[0];
  if (rows.length === 1 && first !== undefined) {
    return `keep ${first.path} (${first.what})?`;
  }
  return `keep ${String(rows.length)} git changes?`;
}

/** The strip of `question`, or `undefined` when it does not wait. */
export function quarantineStrip(
  question: QuarantineQuestionView,
  cwd: string | undefined,
  machine: string,
): QuarantineStripModel | undefined {
  if (question.answer !== "pending") {
    return undefined;
  }
  const line = quarantineLine(question, cwd);
  return {
    callId: question.callId,
    line,
    label: `Git settings changed: ${line} Answer on ${machine}.`,
    terminal: `answer on ${machine}`,
  };
}

/** How `question` ended, in words; `undefined` while it waits. */
export function quarantineAnswer(
  question: QuarantineQuestionView,
  turnEnded: boolean,
  machine: string,
): string | undefined {
  let words: string;
  switch (question.answer) {
    case "kept":
      words = "kept";
      break;
    case "left":
      words = "left in quarantine";
      break;
    case "not_answered":
      return "not answered · left in quarantine";
    default:
      return turnEnded ? "not answered · left in quarantine" : undefined;
  }
  const origin = question.origin;
  return origin !== undefined && TERMINAL_ORIGINS.has(origin) ? `${words} · on ${machine}` : words;
}

/** The sheet of `question`. `turnEnded` is true when its turn has ended. */
export function quarantineSheet(
  question: QuarantineQuestionView,
  cwd: string | undefined,
  machine: string,
  turnEnded: boolean,
): QuarantineSheetModel {
  const closed = quarantineAnswer(question, turnEnded, machine);
  return {
    rows: rowsOf(question, cwd),
    quiet: QUARANTINE_QUIET,
    terminal: closed === undefined ? `answer on ${machine}` : undefined,
    closed,
  };
}
