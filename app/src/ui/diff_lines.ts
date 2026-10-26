// The lines of a unified diff, told apart as efr's terminal does it
// (crates/efr-render/src/code/diff.rs, `Diff::classify`): file headers, hunk headers,
// added, removed and context lines, `\ No newline` notes, git's meta lines and other
// text. A hunk header with counts (`@@ -a,b +c,d @@`) says how many lines its hunk has,
// so a `--- ` or `+++ ` line inside a hunk is a removed or added line, not a file
// header. Model-written diffs often have no usable counts; then the hunk goes on until a
// line that is not a diff line.
//
// On top of the terminal's rules, each line of a hunk gets its old and new line number
// from the header, for the gutter of the phone's diff view.

/** What a line of a diff is. */
export type DiffLineKind =
  | "meta"
  | "file"
  | "hunk"
  | "added"
  | "removed"
  | "context"
  | "note"
  | "other";

/** One line of a diff. */
export interface DiffLine {
  readonly kind: DiffLineKind;
  /** The whole line, without its newline. */
  readonly text: string;
  /** The line number in the old file, for a removed or context line of a hunk with counts. */
  readonly oldLine?: number;
  /** The line number in the new file, for an added or context line of a hunk with counts. */
  readonly newLine?: number;
}

const META = [
  "index ",
  "new file mode",
  "deleted file mode",
  "old mode",
  "new mode",
  "similarity index",
  "dissimilarity index",
  "rename from",
  "rename to",
  "copy from",
  "copy to",
  "Binary files",
];

const FILE_HEADERS = ["--- a/", "--- /dev/null", "+++ b/", "+++ /dev/null"];

interface Hunk {
  /** Lines still expected, old and new; `undefined` when the header had no counts. */
  remaining: { old: number; new: number } | undefined;
  oldLine: number | undefined;
  newLine: number | undefined;
}

/** The lines of `diff`, in order. A final newline gives no empty last line. */
export function diffLines(diff: string): DiffLine[] {
  const source = diff.endsWith("\n") ? diff.slice(0, -1) : diff;
  if (source.length === 0) {
    return [];
  }
  let hunk: Hunk | undefined;
  const out: DiffLine[] = [];
  for (const raw of source.split("\n")) {
    const text = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
    if (text.startsWith("diff --git ")) {
      hunk = undefined;
      out.push({ kind: "meta", text });
      continue;
    }
    if (hunk !== undefined && (hunk.remaining !== undefined || !isFileHeader(text))) {
      const first = text.charAt(0);
      if (first === "@" && text.startsWith("@@")) {
        hunk = newHunk(text);
        out.push({ kind: "hunk", text });
        continue;
      }
      const kind: DiffLineKind | undefined =
        first === "+"
          ? "added"
          : first === "-"
            ? "removed"
            : first === " " || first === ""
              ? "context"
              : first === "\\"
                ? "note"
                : undefined;
      if (kind === undefined) {
        hunk = undefined;
        out.push({ kind: "other", text });
        continue;
      }
      out.push(numbered(hunk, kind, text));
      if (hunk.remaining !== undefined) {
        if (kind !== "added" && kind !== "note") {
          hunk.remaining.old = Math.max(0, hunk.remaining.old - 1);
        }
        if (kind !== "removed" && kind !== "note") {
          hunk.remaining.new = Math.max(0, hunk.remaining.new - 1);
        }
        if (hunk.remaining.old === 0 && hunk.remaining.new === 0) {
          hunk = undefined;
        }
      }
      continue;
    }
    hunk = undefined;
    if (text.startsWith("--- ") || text.startsWith("+++ ")) {
      out.push({ kind: "file", text });
      continue;
    }
    if (text.startsWith("@@")) {
      hunk = newHunk(text);
      out.push({ kind: "hunk", text });
      continue;
    }
    if (META.some((prefix) => text.startsWith(prefix))) {
      out.push({ kind: "meta", text });
      continue;
    }
    const first = text.charAt(0);
    out.push({ kind: first === "+" ? "added" : first === "-" ? "removed" : "other", text });
  }
  return out;
}

/** The number of added and removed lines of `lines`. */
export function diffStat(lines: readonly DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === "added") {
      added++;
    } else if (line.kind === "removed") {
      removed++;
    }
  }
  return { added, removed };
}

function isFileHeader(text: string): boolean {
  return FILE_HEADERS.some((prefix) => text.startsWith(prefix));
}

function newHunk(header: string): Hunk {
  const parsed = hunkRanges(header);
  return {
    remaining: parsed === undefined ? undefined : { old: parsed.oldCount, new: parsed.newCount },
    oldLine: parsed?.oldStart,
    newLine: parsed?.newStart,
  };
}

function numbered(hunk: Hunk, kind: DiffLineKind, text: string): DiffLine {
  if (hunk.oldLine === undefined || hunk.newLine === undefined) {
    return { kind, text };
  }
  switch (kind) {
    case "added":
      return { kind, text, newLine: hunk.newLine++ };
    case "removed":
      return { kind, text, oldLine: hunk.oldLine++ };
    case "context":
      return { kind, text, oldLine: hunk.oldLine++, newLine: hunk.newLine++ };
    default:
      return { kind, text };
  }
}

/** The ranges of `@@ -a,b +c,d @@`; a missing count means 1, as efr's terminal reads it. */
function hunkRanges(
  header: string,
): { oldStart: number; oldCount: number; newStart: number; newCount: number } | undefined {
  const parts = header.replace(/^@+/, "").trim().split(/\s+/);
  const old = range(parts[0], "-");
  const next = range(parts[1], "+");
  if (old === undefined || next === undefined) {
    return undefined;
  }
  return { oldStart: old.start, oldCount: old.count, newStart: next.start, newCount: next.count };
}

/** `-a,b` or `+c` as a start and a count. */
function range(
  part: string | undefined,
  sign: string,
): { start: number; count: number } | undefined {
  if (part === undefined || !part.startsWith(sign)) {
    return undefined;
  }
  const [start, count] = part.slice(1).split(",", 2);
  if (start === undefined || !/^\d+$/.test(start)) {
    return undefined;
  }
  if (count === undefined) {
    return { start: Number(start), count: 1 };
  }
  return /^\d+$/.test(count) ? { start: Number(start), count: Number(count) } : undefined;
}
