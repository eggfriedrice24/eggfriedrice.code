// The texts of an approval block (design 3.11), from the words that efrd sends. Pure, so
// each case has a test.
//
// - The summary. efrd writes `<tool>: <subject>; <subject>` on its first line, and a
//   subject of a command is `run "<line>"` with the quotes and escapes of Rust's Debug
//   form (crates/efr-permissions/src/decision.rs, `Subject`). The block shows the command
//   as `$ <line>` and never cuts it. Other lines and other subjects (such as
//   `asks for: sudo` or `input at the terminal`) stay as quiet detail lines, so the user
//   sees all that efrd asks about.
// - The reason. One plain line; the mode name, `config.toml`, `$SCRATCH` and paths show
//   in mono (the board draws `cautious` and `~/.dotfiles` so).
// - The times. Clock times are the local `HH:MM` of the phone; the time to answer counts
//   down in minutes, then in seconds.

import type { Clock, Timer } from "@/lib/net/clock";
import { shortPath } from "@/features/conversations/model/list-format";

/** What the block shows on its command line. */
export type Subject =
  /** A shell command: `$ <line>`. */
  | { readonly kind: "command"; readonly line: string }
  /** A file write with a diff preview: `edit <path> +2 −1`. */
  | {
      readonly kind: "edit";
      readonly path: string;
      readonly added: number;
      readonly removed: number;
    }
  /** Any other question: the first line of the summary as efrd wrote it. */
  | { readonly kind: "text"; readonly text: string };

/** The parts of an approval summary. */
export interface SummaryParts {
  readonly subject: Subject;
  /** The other subjects and lines of the summary, in order. */
  readonly details: readonly string[];
}

/** The added and removed lines of a diff preview. */
export interface DiffCount {
  readonly added: number;
  readonly removed: number;
}

/** The minus sign (U+2212) of the removed count, as the design draws it. */
export const MINUS = "\u2212";

/** The first line of a question that a job notice started (`WAKE_QUESTION` of efrd). */
const WAKE_PREFIX = "(job notice turn) ";

/** The detail line for a question that a job notice started. */
export const WAKE_DETAIL = "a job notice started this turn";

/** The first line of a question of full mode (`FULL_ASKS` of efrd): a detail line. */
const FULL_PREFIX = "full mode still asks for this: ";

const TOOL_PREFIX = /^([\w.-]+): /;
const WRITE_SUBJECT = /^(?:write|create|edit) (.+?)(?: \([^()]*\))?$/;

/**
 * The parts of `summary`. `diff` is the count of the diff preview, when the question has
 * one: a write subject then becomes an edit line.
 */
export function summaryParts(summary: string, diff: DiffCount | undefined): SummaryParts {
  const details: string[] = [];
  let subject: Subject | undefined;
  for (const raw of summary.split("\n")) {
    let line = raw;
    if (line.startsWith(WAKE_PREFIX)) {
      details.push(WAKE_DETAIL);
      line = line.slice(WAKE_PREFIX.length);
    }
    if (line.length === 0) {
      continue;
    }
    if (subject !== undefined || line.startsWith(FULL_PREFIX)) {
      details.push(line);
      continue;
    }
    subject = mainSubject(line, diff, details);
  }
  return { subject: subject ?? { kind: "text", text: details.shift() ?? summary }, details };
}

/**
 * The subject of the first summary line `line`: its command, else its write when the
 * question has a diff, else the whole line. The other subjects of the line go to
 * `details`.
 */
function mainSubject(line: string, diff: DiffCount | undefined, details: string[]): Subject {
  const parts = subjectsOf(line);
  if (parts === undefined) {
    return { kind: "text", text: line };
  }
  let main = parts.findIndex((part) => part.command !== undefined);
  if (main === -1 && diff !== undefined) {
    main = parts.findIndex((part) => writePath(part.text) !== undefined);
  }
  const part = parts[main];
  if (part === undefined) {
    return { kind: "text", text: line };
  }
  for (const [index, other] of parts.entries()) {
    if (index !== main) {
      details.push(other.text);
    }
  }
  if (part.command !== undefined) {
    return { kind: "command", line: part.command };
  }
  return {
    kind: "edit",
    path: shortPath(writePath(part.text) ?? part.text),
    added: diff?.added ?? 0,
    removed: diff?.removed ?? 0,
  };
}

/** The one-line form of `subject`, as the quiet line of an answered block shows it. */
export function subjectLine(subject: Subject): string {
  if (subject.kind === "command") {
    return `$ ${subject.line}`;
  }
  if (subject.kind === "edit") {
    return `edit ${subject.path} +${subject.added} ${MINUS}${subject.removed}`;
  }
  return subject.text;
}

/** One subject of a summary line. */
interface SubjectPart {
  /** The subject as efrd wrote it. */
  readonly text: string;
  /** The command line, for a `run "..."` subject. */
  readonly command: string | undefined;
}

/**
 * The subjects of a summary line `<tool>: <a>; <b>`, or `undefined` when the line does
 * not have that form. A command subject holds its line in Debug quotes, so a `; ` inside
 * the quotes does not split it.
 */
function subjectsOf(line: string): SubjectPart[] | undefined {
  const prefix = TOOL_PREFIX.exec(line);
  if (prefix === null) {
    return undefined;
  }
  const parts: SubjectPart[] = [];
  let at = prefix[0].length;
  while (at < line.length) {
    if (line.startsWith('run "', at)) {
      const quoted = debugString(line, at + 4);
      if (quoted === undefined) {
        return undefined;
      }
      const end = line.indexOf("; ", quoted.end);
      const stop = end === -1 ? line.length : end;
      const tail = line.slice(quoted.end, stop);
      // NOTE: a subject such as `run "x" as a job (kind)` is not a plain command line.
      parts.push({
        text: line.slice(at, stop),
        command: tail.length === 0 ? quoted.value : undefined,
      });
      at = end === -1 ? line.length : end + 2;
      continue;
    }
    const end = line.indexOf("; ", at);
    const stop = end === -1 ? line.length : end;
    parts.push({ text: line.slice(at, stop), command: undefined });
    at = end === -1 ? line.length : end + 2;
  }
  return parts;
}

/** The path of a write subject such as `write /home/me/notes.md (user data)`. */
function writePath(text: string): string | undefined {
  return WRITE_SUBJECT.exec(text)?.[1];
}

/**
 * The value of the Rust Debug string that starts with the quote at `start` of `text`,
 * and the index after its closing quote; `undefined` when the string does not close or
 * has an escape that Debug does not write.
 */
export function debugString(
  text: string,
  start: number,
): { readonly value: string; readonly end: number } | undefined {
  if (text.charAt(start) !== '"') {
    return undefined;
  }
  let value = "";
  let at = start + 1;
  while (at < text.length) {
    const char = text.charAt(at);
    if (char === '"') {
      return { value, end: at + 1 };
    }
    if (char !== "\\") {
      value += char;
      at += 1;
      continue;
    }
    const next = text.charAt(at + 1);
    const simple = ESCAPES[next];
    if (simple !== undefined) {
      value += simple;
      at += 2;
      continue;
    }
    if (next === "u" && text.charAt(at + 2) === "{") {
      const close = text.indexOf("}", at + 3);
      const code = close === -1 ? Number.NaN : Number.parseInt(text.slice(at + 3, close), 16);
      if (!Number.isInteger(code) || code < 0 || code > 0x10ffff) {
        return undefined;
      }
      value += String.fromCodePoint(code);
      at = close + 1;
      continue;
    }
    return undefined;
  }
  return undefined;
}

const ESCAPES: Readonly<Record<string, string>> = {
  '"': '"',
  "'": "'",
  "\\": "\\",
  n: "\n",
  r: "\r",
  t: "\t",
  "0": "\0",
};

/** A part of the reason line: plain words, or a name that shows in mono. */
export interface ReasonSpan {
  /** The offset of the span in the reason, its key. */
  readonly at: number;
  readonly text: string;
  readonly mono: boolean;
}

// A name that shows in mono: the mode before "asks", "mode still asks" or "sandbox";
// config.toml; $SCRATCH (group 1). Or a path that starts with `~` or `/` (group 3) after
// a space, a "(" or the start (group 2). A path ends before a last "." or ",", which
// ends the sentence. Hermes gets no lookbehind here, so the prefix is a group.
const MONO_PART =
  /(\b(?:manual|cautious|auto|full)(?= (?:asks|mode still asks|sandbox))|config\.toml|\$SCRATCH)|(^|[\s(])(~(?:\/(?:[\w@%+=:-]|[./](?=[\w@%+=:.~-]))*)?|\/(?:[\w@%+=:-]|[./](?=[\w@%+=:.~-]))+)/g;

/** The spans of `reason`, in order. */
export function reasonSpans(reason: string): ReasonSpan[] {
  const spans: ReasonSpan[] = [];
  let at = 0;
  for (const match of reason.matchAll(MONO_PART)) {
    const name = match[1] ?? match[3];
    if (name === undefined) {
      continue;
    }
    const start = match.index + (match[1] === undefined ? (match[2] ?? "").length : 0);
    if (start > at) {
      spans.push({ at, text: reason.slice(at, start), mono: false });
    }
    spans.push({ at: start, text: name, mono: true });
    at = start + name.length;
  }
  if (at < reason.length) {
    spans.push({ at, text: reason.slice(at), mono: false });
  }
  return spans;
}

/** The header text when the time to answer is over but efrd did not close the question yet. */
export const TIME_UP = "time is up";

const MINUTE = 60_000;
const SECOND = 1000;

/** The last time left that the countdown shows in seconds. */
const LAST_SECONDS = 59 * SECOND;

/**
 * The header text of a question that stops waiting at `expiresAt`, at `now` (both ms
 * since the epoch): `expires in 4m` (whole minutes left, at least 1), then from 59
 * seconds on `expires in 45s`, then `time is up`.
 */
export function expiryLabel(expiresAt: number, now: number): string {
  const left = expiresAt - now;
  if (left <= 0) {
    return TIME_UP;
  }
  if (left > LAST_SECONDS) {
    return `expires in ${Math.max(1, Math.floor(left / MINUTE))}m`;
  }
  return `expires in ${Math.ceil(left / SECOND)}s`;
}

/**
 * The first time after `now` at which `expiryLabel` gives another text, or `undefined`
 * when it will not change again.
 */
export function nextExpiryChange(expiresAt: number, now: number): number | undefined {
  const left = expiresAt - now;
  if (left <= 0) {
    return undefined;
  }
  if (left >= 2 * MINUTE) {
    // NOTE: the whole minutes drop 1 ms after a minute boundary.
    return now + (left % MINUTE) + 1;
  }
  if (left > LAST_SECONDS) {
    return now + (left - LAST_SECONDS);
  }
  // NOTE: the seconds round up, so they drop at each whole second.
  return now + ((left - 1) % SECOND) + 1;
}

/**
 * Calls `onChange` each time `expiryLabel(expiresAt, clock.now())` gives a new text, with
 * one timer at a time. It returns the function that stops it.
 */
export function watchExpiry(expiresAt: number, clock: Clock, onChange: () => void): () => void {
  let timer: Timer | undefined;
  const arm = (): void => {
    const now = clock.now();
    const next = nextExpiryChange(expiresAt, now);
    timer =
      next === undefined
        ? undefined
        : clock.setTimer(next - now, () => {
            onChange();
            arm();
          });
  };
  arm();
  return () => timer?.cancel();
}
