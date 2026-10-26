// The block splitter of a streamed message (R4c.4 step 6 of the research, the rule of
// Vercel's streamdown): a message is a list of final blocks plus one tail. A block is
// final once the next block has begun, and a final block never changes, whatever text
// comes after it. So a long reply can show each final block in a view of its own that
// never renders again, and only the tail's view changes while the model writes.
//
// The rules, outside a fenced code block:
//
// - A blank line ends the content of a block. The next line that starts in column 0
//   begins the next block. A line that starts with a space or a tab after a blank line
//   continues the block (a list item or a quote that goes on).
// - A fence (three or more backticks or tildes) in column 0 begins a block, also right
//   after a paragraph line, because a fence ends a paragraph in CommonMark. The line
//   after its closing fence begins the next block.
// - A blank line inside a fence ends nothing.
//
// A fence opens only on a whole line (one with its newline), because the rest of the
// line decides whether it is a fence. A block whose only content is a closed fence with
// the info `diff`, `patch` or `udiff` is a diff block, for the diff view.
//
// The split of a longer text continues the split of a shorter one: `splitBlocks(text,
// previous)` scans only the previous tail when `text` extends `previous.text`. Offsets are
// UTF-16 indexes into the text.

/** One final block: `text.slice(start, end)`, with the blank lines after it. */
export interface Block {
  readonly start: number;
  readonly end: number;
  readonly kind: "markdown" | "diff";
}

/** A text as final blocks and a tail. */
export interface BlockSplit {
  readonly text: string;
  readonly blocks: readonly Block[];
  /** Where the tail begins: the end of the last final block. */
  readonly tailStart: number;
}

/** The split of the empty text. */
export const EMPTY_SPLIT: BlockSplit = { text: "", blocks: [], tailStart: 0 };

const DIFF_INFO = new Set(["diff", "patch", "udiff"]);

interface Fence {
  readonly marker: string;
  readonly length: number;
}

/** Splits `text`; with `previous`, continues it when `text` extends its text. */
export function splitBlocks(text: string, previous: BlockSplit = EMPTY_SPLIT): BlockSplit {
  const resume =
    text.length >= previous.text.length &&
    text.startsWith(previous.text.slice(previous.tailStart), previous.tailStart)
      ? previous
      : EMPTY_SPLIT;
  if (resume.text === text) {
    return resume;
  }
  const blocks = [...resume.blocks];
  const scan = scanFrom(text, resume.tailStart, (block) => blocks.push(block));
  return { text, blocks, tailStart: scan.blockStart };
}

/**
 * The blocks of a whole message: the final blocks, then the tail as one more block. The
 * text ends here, so its last line counts as a whole line.
 */
export function completeBlocks(text: string, previous: BlockSplit = EMPTY_SPLIT): Block[] {
  const split = splitBlocks(text, previous);
  const blocks = [...split.blocks];
  if (split.tailStart >= text.length) {
    return blocks;
  }
  const scan = scanFrom(`${text}\n`, split.tailStart, (block) => blocks.push(block));
  if (scan.blockStart < text.length) {
    blocks.push({
      start: scan.blockStart,
      end: text.length,
      kind: scan.diff ? "diff" : "markdown",
    });
  }
  return blocks;
}

/** The lines inside the fences of a diff block, as one text. */
export function fenceBody(block: string): string {
  const lines = block.replace(/\n+$/, "").split("\n");
  return lines.slice(1, -1).join("\n");
}

interface ScanEnd {
  readonly blockStart: number;
  /** The open block is a closed diff fence. */
  readonly diff: boolean;
}

function scanFrom(text: string, from: number, push: (block: Block) => void): ScanEnd {
  let blockStart = from;
  let hasContent = false;
  let sawBlank = false;
  let fence: Fence | undefined;
  let closedFence = false;
  let fenceIsDiff = false;
  let blockIsFence = false;

  let lineStart = from;
  while (lineStart < text.length) {
    const newline = text.indexOf("\n", lineStart);
    const whole = newline >= 0;
    const lineEnd = whole ? newline : text.length;
    const line = text.slice(lineStart, lineEnd);
    const next = whole ? newline + 1 : text.length;

    if (fence !== undefined) {
      if (whole && closesFence(line, fence)) {
        fence = undefined;
        closedFence = true;
      }
      lineStart = next;
      continue;
    }
    if (/^[ \t]*\r?$/.test(line)) {
      if (hasContent) {
        sawBlank = true;
      }
      lineStart = next;
      continue;
    }
    const column0 = line[0] !== " " && line[0] !== "\t";
    const opens = whole ? opensFence(line) : undefined;
    if (hasContent && (closedFence || (column0 && sawBlank) || (column0 && opens !== undefined))) {
      push({
        start: blockStart,
        end: lineStart,
        kind: blockIsFence && fenceIsDiff && closedFence ? "diff" : "markdown",
      });
      blockStart = lineStart;
      hasContent = false;
      blockIsFence = false;
      fenceIsDiff = false;
    }
    if (opens !== undefined) {
      fence = opens.fence;
    }
    // Only a block that begins with a fence in column 0 can be a diff block; a fence in a
    // list item stays in the list's block.
    blockIsFence = opens !== undefined && !hasContent && column0;
    fenceIsDiff = blockIsFence && opens?.diff === true;
    hasContent = true;
    sawBlank = false;
    closedFence = false;
    lineStart = next;
  }
  return { blockStart, diff: blockIsFence && fenceIsDiff && closedFence };
}

function opensFence(line: string): { fence: Fence; diff: boolean } | undefined {
  const match = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line.replace(/\r$/, ""));
  if (match === null) {
    return undefined;
  }
  const marker = match[1] ?? "";
  const info = (match[2] ?? "").trim();
  if (marker.startsWith("`") && info.includes("`")) {
    return undefined;
  }
  const word = info.split(/\s+/, 1)[0]?.toLowerCase() ?? "";
  return {
    fence: { marker: marker.charAt(0), length: marker.length },
    diff: DIFF_INFO.has(word),
  };
}

function closesFence(line: string, fence: Fence): boolean {
  const match = /^ {0,3}(`{3,}|~{3,})[ \t]*\r?$/.exec(line);
  const marker = match?.[1];
  return marker !== undefined && marker.charAt(0) === fence.marker && marker.length >= fence.length;
}
