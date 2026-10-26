// The model of an edit block and of every diff view (design 3.8 and the S-Diff sheet),
// over the lines of diff-lines.ts:
//
// - The header: the path, and the added and removed counts (`+14`, `−3`).
// - Collapsed: a preview of 4 lines from one line of context before the first change,
//   with one number column, then "show full diff · 3 hunks" ("· 1 hunk", "· new file").
// - Expanded: each hunk with its header and two number columns, old and new. A diff
//   over 400 lines shows its first 3 hunks, then "show remaining 9 hunks".
// - A changed run: when removed lines and added lines pair up, the part of each pair
//   that differs is bold and bright. Only text colours exist, no fill (design note 10).
//
// The number column is 28 pt wide in every diff (design item 8.9, theme DIFF_GUTTER).

import { type DiffLine, diffLines } from "./diff-lines";

/** From this many lines, an expanded diff shows its first hunks only. */
export const BIG_DIFF_LINES = 400;

/** The hunks that a big diff shows before "show remaining". */
export const SHOWN_HUNKS = 3;

/** The lines of a collapsed preview. */
export const PREVIEW_LINES = 4;

/** A part of a diff line: plain, or the changed run of a pair. */
export interface DiffSpan {
  /** Where the span begins in the line's text: its key in a view. */
  readonly start: number;
  readonly text: string;
  readonly changed: boolean;
}

/** A line of a diff with its spans. */
export interface DiffRow {
  /** The place of the line in the diff: its key in a view, because a diff never reorders. */
  readonly id: number;
  readonly line: DiffLine;
  /** The text in parts; one plain span when nothing is marked. */
  readonly spans: readonly DiffSpan[];
}

/** One hunk: its `@@` header and its lines. */
export interface DiffHunk {
  /** The place of the hunk in the diff. */
  readonly id: number;
  /** The `@@ -a,b +c,d @@` line; undefined for lines before the first header. */
  readonly header: string | undefined;
  readonly rows: readonly DiffRow[];
}

/** A diff, ready for a view. */
export interface DiffModel {
  readonly path: string;
  readonly newFile: boolean;
  readonly added: number;
  readonly removed: number;
  readonly hunks: readonly DiffHunk[];
  /** The number of lines in the hunks. */
  readonly lineCount: number;
}

/** The model of `diff`. `fallbackPath` names the file when the diff has no file header. */
export function diffModel(diff: string, fallbackPath: string): DiffModel {
  const lines = diffLines(diff);
  let path: string | undefined;
  let oldPath: string | undefined;
  let newFile = false;
  let added = 0;
  let removed = 0;
  const hunks: { header: string | undefined; lines: DiffLine[] }[] = [];
  let current: { header: string | undefined; lines: DiffLine[] } | undefined;
  for (const line of lines) {
    switch (line.kind) {
      case "file":
        if (line.text.startsWith("+++ ")) {
          const name = line.text.slice(4).trim();
          if (name !== "/dev/null") {
            path = stripPrefix(name, "b/");
          }
        } else {
          const name = line.text.slice(4).trim();
          if (name === "/dev/null") {
            newFile = true;
          } else {
            oldPath = stripPrefix(name, "a/");
          }
        }
        continue;
      case "meta":
        if (line.text.startsWith("new file mode")) {
          newFile = true;
        }
        continue;
      case "hunk":
        current = { header: line.text, lines: [] };
        hunks.push(current);
        continue;
      case "added":
        added++;
        break;
      case "removed":
        removed++;
        break;
      default:
        break;
    }
    if (current === undefined) {
      current = { header: undefined, lines: [] };
      hunks.push(current);
    }
    current.lines.push(line);
  }
  let next = 0;
  const built = hunks.map((hunk, id): DiffHunk => {
    const rows = withRuns(hunk.lines, next);
    next += rows.length;
    return { id, header: hunk.header, rows };
  });
  return {
    path: path ?? oldPath ?? fallbackPath,
    newFile,
    added,
    removed,
    hunks: built,
    lineCount: built.reduce((sum, hunk) => sum + hunk.rows.length, 0),
  };
}

function stripPrefix(name: string, prefix: string): string {
  return name.startsWith(prefix) ? name.slice(prefix.length) : name;
}

/** The rows of a collapsed preview: 4 lines from one line of context before the first change. */
export function previewRows(model: DiffModel): DiffRow[] {
  const rows = model.hunks.flatMap((hunk) => hunk.rows);
  const first = rows.findIndex((row) => row.line.kind === "added" || row.line.kind === "removed");
  const start = first <= 0 ? 0 : rows[first - 1]?.line.kind === "context" ? first - 1 : first;
  return rows.slice(start, start + PREVIEW_LINES);
}

/** The hunks that an expanded diff shows, and how many it holds back. */
export function shownHunks(
  model: DiffModel,
  all: boolean,
): { readonly hunks: readonly DiffHunk[]; readonly hidden: number } {
  if (all || model.lineCount <= BIG_DIFF_LINES || model.hunks.length <= SHOWN_HUNKS) {
    return { hunks: model.hunks, hidden: 0 };
  }
  return { hunks: model.hunks.slice(0, SHOWN_HUNKS), hidden: model.hunks.length - SHOWN_HUNKS };
}

/** The label of the expand button: `show full diff · 3 hunks`, `· 1 hunk`, `· new file`. */
export function expandLabel(model: DiffModel): string {
  if (model.newFile) {
    return "show full diff · new file";
  }
  const count = model.hunks.filter((hunk) => hunk.header !== undefined).length;
  return `show full diff · ${String(count)} ${count === 1 ? "hunk" : "hunks"}`;
}

/** The label of the button of a big diff: `show remaining 9 hunks`. */
export function remainingLabel(hidden: number): string {
  return `show remaining ${String(hidden)} ${hidden === 1 ? "hunk" : "hunks"}`;
}

/** The counts of the header: `+14` and `−3` (with U+2212, as the design draws it). */
export function statTexts(model: DiffModel): { readonly added: string; readonly removed: string } {
  return { added: `+${String(model.added)}`, removed: `−${String(model.removed)}` };
}

/** The number of the single column of a preview: new for added and context, old for removed. */
export function previewNumber(line: DiffLine): number | undefined {
  return line.kind === "removed" ? line.oldLine : (line.newLine ?? line.oldLine);
}

/** The VoiceOver label of a line: "added", "removed" or the text alone. */
export function lineLabel(line: DiffLine): string {
  const text = line.kind === "added" || line.kind === "removed" ? line.text.slice(1) : line.text;
  if (line.kind === "added") {
    return `added: ${text}`;
  }
  if (line.kind === "removed") {
    return `removed: ${text}`;
  }
  return text;
}

/** The VoiceOver label of a diff's header. */
export function diffLabel(model: DiffModel): string {
  return `Edit ${model.path}, ${String(model.added)} lines added, ${String(model.removed)} lines removed`;
}

/**
 * The rows of the lines of one hunk, with the changed runs of the pairs marked. The ids
 * count on from `firstId`.
 */
function withRuns(lines: readonly DiffLine[], firstId: number): DiffRow[] {
  const rows: DiffRow[] = lines.map((line, index) => ({
    id: firstId + index,
    line,
    spans: [{ start: 0, text: line.text, changed: false }],
  }));
  let i = 0;
  while (i < lines.length) {
    if (lines[i]?.kind !== "removed") {
      i++;
      continue;
    }
    const removedStart = i;
    while (lines[i]?.kind === "removed") {
      i++;
    }
    const addedStart = i;
    while (lines[i]?.kind === "added") {
      i++;
    }
    const pairs = Math.min(addedStart - removedStart, i - addedStart);
    for (let pair = 0; pair < pairs; pair++) {
      const removed = lines[removedStart + pair];
      const added = lines[addedStart + pair];
      if (removed === undefined || added === undefined) {
        continue;
      }
      const runs = changedRun(removed.text.slice(1), added.text.slice(1));
      if (runs !== undefined) {
        rows[removedStart + pair] = {
          id: firstId + removedStart + pair,
          line: removed,
          spans: spansOf(removed.text, runs.old),
        };
        rows[addedStart + pair] = {
          id: firstId + addedStart + pair,
          line: added,
          spans: spansOf(added.text, runs.new),
        };
      }
    }
  }
  return rows;
}

interface Run {
  readonly start: number;
  readonly end: number;
}

/**
 * The changed part of a pair: what lies between the common start and the common end.
 * Undefined when the two have nothing in common or when they are the same.
 */
function changedRun(before: string, after: string): { old: Run; new: Run } | undefined {
  let prefix = 0;
  const shorter = Math.min(before.length, after.length);
  while (prefix < shorter && before.charCodeAt(prefix) === after.charCodeAt(prefix)) {
    prefix++;
  }
  let suffix = 0;
  while (
    suffix < shorter - prefix &&
    before.charCodeAt(before.length - 1 - suffix) === after.charCodeAt(after.length - 1 - suffix)
  ) {
    suffix++;
  }
  const common = prefix + suffix;
  if (common === 0 || (before.length === after.length && common >= before.length)) {
    return undefined;
  }
  // The text's first character is the + or - sign, so each run moves on by one.
  return {
    old: wordRun(before, prefix, before.length - suffix, 1),
    new: wordRun(after, prefix, after.length - suffix, 1),
  };
}

const LETTER_OR_DIGIT = /[\p{L}\p{N}]/u;

/**
 * The run from `start` to `end` of `text`, grown to whole words (design: the changed
 * run is word level), and moved on by `shift`. Only letters and digits make a word, so
 * `_` and `.` end one: `is_dropped` marks `dropped`, not the whole name.
 */
function wordRun(text: string, start: number, end: number, shift: number): Run {
  let from = start;
  while (
    from > 0 &&
    LETTER_OR_DIGIT.test(text.charAt(from - 1)) &&
    LETTER_OR_DIGIT.test(text.charAt(from))
  ) {
    from--;
  }
  let to = end;
  while (
    to < text.length &&
    to > 0 &&
    LETTER_OR_DIGIT.test(text.charAt(to)) &&
    LETTER_OR_DIGIT.test(text.charAt(to - 1))
  ) {
    to++;
  }
  return { start: from + shift, end: to + shift };
}

function spansOf(text: string, run: Run): DiffSpan[] {
  const spans: DiffSpan[] = [];
  if (run.start > 0) {
    spans.push({ start: 0, text: text.slice(0, run.start), changed: false });
  }
  if (run.end > run.start) {
    spans.push({ start: run.start, text: text.slice(run.start, run.end), changed: true });
  }
  if (run.end < text.length) {
    spans.push({ start: run.end, text: text.slice(run.end), changed: false });
  }
  return spans;
}
