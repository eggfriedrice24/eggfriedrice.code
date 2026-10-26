// The segments of an assistant message: what the message row draws, each in a view of
// its own (plan section 2.7 and decision D12). Enriched 1.1.1 draws no code block
// header, so each fenced code block becomes a segment that our code block frame holds
// (language label, "streaming", copy), and the Markdown between the fences becomes
// Markdown segments.
//
// It reads the block split of blocks.ts, so a block that the splitter called final
// never changes its segment:
//
// - A final block that begins with a fence is a code segment, or a diff segment for a
//   closed `diff`, `patch` or `udiff` fence. The splitter begins a new block at each
//   fence, so such a block holds only the fence.
// - The other final blocks are Markdown. A short message joins the Markdown between two
//   fences into one segment, so the text reads as one view. A long message (from
//   BLOCKS_FROM) gives each final block its own segment, so a frame parses only the
//   tail (research R4c.4 step 6).
// - The tail is the part that still grows. A tail that begins with a fence is an open
//   code segment: it shows as a block as soon as the fence opens, never as raw
//   backticks (design 3.5). A last line that could still become a fence waits until its
//   newline comes, for the same reason.
//
// A segment's key is the offset where it begins, so a segment keeps its view while it
// grows, and an open fence keeps its view when it closes.
//
// The text of a Markdown segment is inert (inertMarkdown): enriched 1.1.1 loads each
// image URL with no tap, and makes a `<video src>` HTML block into a video. A reply of
// the model could send a request from the phone with them. So no image and no HTML block
// stays in the Markdown.

import { type BlockSplit, completeBlocks } from "./blocks";

/** From this length (UTF-16 units) each final block of a message is a segment of its own. */
export const BLOCKS_FROM = 20_000;

/** One segment of a message. */
export type Segment =
  | {
      readonly kind: "markdown";
      readonly key: string;
      readonly text: string;
      /** True for the segment that still grows. */
      readonly open: boolean;
    }
  | {
      readonly kind: "code";
      readonly key: string;
      /** The first word of the fence's info, in lower case; "" when there is none. */
      readonly language: string;
      /** The whole fence as Markdown, for the highlighter. */
      readonly source: string;
      /** The lines inside the fence: what the copy button copies. */
      readonly body: string;
      /** False while the fence is still open: copy is off, and the label says "streaming". */
      readonly closed: boolean;
    }
  | {
      readonly kind: "diff";
      readonly key: string;
      /** The lines inside the fence. */
      readonly body: string;
    };

const FENCE_LINE = /^ {0,3}(`{3,}|~{3,})(.*)$/;

/** U+2060 WORD JOINER: it has no width, so a code span still shows its text as it is. */
const WORD_JOINER = "\u2060";

/** A `<` that begins the content of a line: after spaces, `>` and list markers. */
const LINE_START_TAG = /^((?:[ \t]*(?:>|[-+*](?=[ \t])|\d{1,9}[.)](?=[ \t])))*[ \t]*)</gm;

/**
 * `markdown` with no image and no HTML block, for enriched.
 *
 * - `![alt](url)` becomes `!` and the link `[alt](url)`: a tap asks first, as for each
 *   link (links.ts). A word joiner goes between `!` and `[`.
 * - A line that begins with `<` cannot begin an HTML block: a word joiner goes before
 *   the `<`. Inline HTML is text in enriched already.
 *
 * It does not look for code spans: a wrong guess would keep an image. The word joiner
 * has no width, so a code span such as `vec![1]` looks the same. The text only grows
 * when `markdown` only grows, so the fade of the tail stays aligned.
 */
export function inertMarkdown(markdown: string): string {
  return markdown.replaceAll("![", `!${WORD_JOINER}[`).replace(LINE_START_TAG, `$1${WORD_JOINER}<`);
}

/** The fence languages of a diff, as blocks.ts knows them. */
const DIFF_LANGUAGES: ReadonlySet<string> = new Set(["diff", "patch", "udiff"]);

/** The language of a fence's first line, or undefined when the line opens no fence. */
export function fenceLanguage(line: string): string | undefined {
  const match = FENCE_LINE.exec(line.replace(/\r$/, ""));
  if (match === null) {
    return undefined;
  }
  const marker = match[1] ?? "";
  const info = (match[2] ?? "").trim();
  if (marker.startsWith("`") && info.includes("`")) {
    return undefined;
  }
  return (info.split(/\s+/, 1)[0] ?? "").toLowerCase();
}

/** True when `line`, a line without its newline yet, can still become a fence. */
function maybeFence(line: string): boolean {
  const match = /^ {0,3}(`+|~+)(.*)$/.exec(line);
  if (match === null) {
    return false;
  }
  const marker = match[1] ?? "";
  const rest = match[2] ?? "";
  return rest === "" || (marker.length >= 3 && !(marker.startsWith("`") && rest.includes("`")));
}

function firstLine(text: string): string {
  const newline = text.indexOf("\n");
  return newline < 0 ? text : text.slice(0, newline);
}

/**
 * The fence of `source`, a text that begins with a fence line: the lines inside it, and
 * whether a closing line came. Only a whole line can close the fence; with `lastIsWhole`
 * the last line of `source` counts as whole without its newline.
 */
function parseFence(source: string, lastIsWhole: boolean): { body: string; closed: boolean } {
  const lines = source.split("\n");
  const marker = FENCE_LINE.exec(lines[0] ?? "")?.[1] ?? "```";
  const wholeLines = lastIsWhole ? lines.length : lines.length - 1;
  const body: string[] = [];
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i] ?? "";
    const close = /^ {0,3}(`{3,}|~{3,})[ \t]*\r?$/.exec(line)?.[1];
    if (
      i < wholeLines &&
      close !== undefined &&
      close.charAt(0) === marker.charAt(0) &&
      close.length >= marker.length
    ) {
      return { body: body.join("\n"), closed: true };
    }
    body.push(line);
  }
  return { body: body.join("\n").replace(/\n+$/, ""), closed: false };
}

/**
 * The segments that no later text can change, by the range they come from. One message
 * view keeps one, so a frame does not slice and parse its final blocks again, and a
 * final segment keeps its object, which lets its view skip the render.
 */
export type SegmentCache = Map<string, Segment>;

/**
 * The segments of `text`. `split` is the block split of `text` (splitBlocks of
 * blocks.ts); `final` is true when the text cannot grow any more. `cache` keeps the
 * final segments from one call to the next; it belongs to one generation of a message.
 */
export function messageSegments(
  text: string,
  split: BlockSplit,
  final: boolean,
  cache: SegmentCache = new Map(),
): Segment[] {
  const perBlock = text.length >= BLOCKS_FROM;
  const blocks = final ? completeBlocks(text, split) : split.blocks;
  const segments: Segment[] = [];
  let run: { start: number; end: number } | undefined;

  const kept = (range: string, build: () => Segment): Segment => {
    const held = cache.get(range);
    if (held !== undefined) {
      return held;
    }
    const built = build();
    cache.set(range, built);
    return built;
  };
  const flush = (open: boolean): void => {
    if (run !== undefined) {
      const { start, end } = run;
      run = undefined;
      const build = (): Segment => ({
        kind: "markdown",
        key: `m${String(start)}`,
        text: inertMarkdown(text.slice(start, end).replace(/\n+$/, "")),
        open,
      });
      const segment = open ? build() : kept(`m${String(start)}:${String(end)}`, build);
      if (segment.kind === "markdown" && segment.text.length > 0) {
        segments.push(segment);
      }
    }
  };
  const markdown = (start: number, end: number): void => {
    if (perBlock) {
      flush(false);
    }
    run = run === undefined ? { start, end } : { start: run.start, end };
  };

  // A closed fence never changes, so its segment is kept by where it begins: the tail's
  // segment of a fence that just closed is the same object as its final block's.
  const fence = (at: number, source: string, language: string, whole: boolean): Segment => {
    const key = `c${String(at)}`;
    const held = cache.get(key);
    if (held !== undefined) {
      return held;
    }
    const parsed = parseFence(source, whole);
    // A final block is whole: a fence that the message never closed ends with it.
    const closed = parsed.closed || whole;
    const segment: Segment =
      parsed.closed && DIFF_LANGUAGES.has(language)
        ? { kind: "diff", key, body: parsed.body }
        : {
            kind: "code",
            key,
            language,
            source: source.replace(/\n+$/, ""),
            body: parsed.body,
            closed,
          };
    if (closed) {
      cache.set(key, segment);
    }
    return segment;
  };

  for (const block of blocks) {
    const source = text.slice(block.start, block.end);
    const language = fenceLanguage(firstLine(source));
    if (language === undefined) {
      markdown(block.start, block.end);
      continue;
    }
    flush(false);
    segments.push(fence(block.start, source, language, true));
  }

  if (final) {
    flush(false);
    return segments;
  }
  const tailStart = split.tailStart;
  const tail = text.slice(tailStart);
  if (tail.length === 0) {
    flush(false);
    return segments;
  }
  const tailLanguage = tail.includes("\n") ? fenceLanguage(firstLine(tail)) : undefined;
  if (tailLanguage !== undefined) {
    flush(false);
    segments.push(fence(tailStart, tail, tailLanguage, false));
    return segments;
  }
  // A last line that can still become a fence waits for its newline.
  const lastLineStart = tail.lastIndexOf("\n") + 1;
  const lastLine = tail.slice(lastLineStart);
  if (maybeFence(lastLine)) {
    if (lastLineStart > 0) {
      markdown(tailStart, tailStart + lastLineStart);
    }
    flush(true);
    const language = fenceLanguage(lastLine) ?? "";
    segments.push(fence(tailStart + lastLineStart, lastLine, language, false));
    return segments;
  }
  markdown(tailStart, text.length);
  flush(true);
  return segments;
}

/** The label of a code block header: `rust`, or `rust · streaming` while the fence is open. */
export function codeLabel(language: string, closed: boolean): string {
  const name = language === "" ? "code" : language;
  return closed ? name : `${name} · streaming`;
}
