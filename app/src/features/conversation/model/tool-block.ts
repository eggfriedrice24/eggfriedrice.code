// The words of a tool block (design 3.7 and the S-Tools sheet): the header (a verb in
// text_subtle and the subject in text_bright), the status (a glyph and a number, never
// colour alone), and the lines of the output.
//
// - The verb comes from the tool: `$` for a shell command, `read` for a file read,
//   `edit` for a file tool. Another tool shows its own name.
// - Done: `✓ 0 · 38s` (exit code and time). A read says how many lines it gave
//   (`✓ 61 lines`). Failed: `✗ 101 · 9s`. A call that its turn left behind, such as at an
//   interrupt, failed too (`✗ 130` when efrd gave the code).
// - Collapsed, the block shows the last 3 lines of the output; expanded, the whole
//   output, at most 200 lines. Each line has a key from its byte offset in the whole
//   output, so a line keeps its view (and fades in only once) while the output grows.
// - While the call's approval waits, the status is "waiting for approval", with no time.
// - The word "error" in an output line is red (design: stderr "error" words).

import type { ToolCallView, ToolInputView } from "@/lib/stream/reducer";
import { utf8Length } from "@/lib/stream/utf8";
import { between, elapsed } from "./durations";
import { size } from "./format";

/** The lines of output of a collapsed block. */
export const TAIL_LINES = 3;

/** The most lines of output of an expanded block (design 3.7). */
export const MAX_OUTPUT_LINES = 200;

/** The tools that change files: their calls have a diff and show as an edit block. */
export const FILE_TOOLS: ReadonlySet<string> = new Set(["apply_patch", "edit", "write_file"]);

/** The header of a tool block. */
export interface ToolHeader {
  /** `$`, `read`, `edit`, or the tool's name. */
  readonly verb: string;
  readonly subject: string;
  /** A shell command wraps; a path or another subject is cut in the middle. */
  readonly wraps: boolean;
}

/** The header of `call`. */
export function toolHeader(call: Pick<ToolCallView, "tool" | "summary">): ToolHeader {
  switch (call.tool) {
    case "shell":
      return { verb: "$", subject: call.summary, wraps: true };
    case "read_file":
      return { verb: "read", subject: call.summary, wraps: false };
    default:
      return {
        verb: FILE_TOOLS.has(call.tool) ? "edit" : call.tool === "" ? "tool" : call.tool,
        subject: call.summary === call.tool ? "" : call.summary,
        wraps: false,
      };
  }
}

/** The status of a tool block: what runs, or how it ended. */
export type ToolStatus =
  | { readonly kind: "waiting" }
  | { readonly kind: "running"; readonly elapsed: string | undefined }
  | { readonly kind: "done"; readonly detail: string }
  | { readonly kind: "failed"; readonly detail: string };

/** The approval question of a call, as its tool block needs it. */
export interface ToolApproval {
  /** The question still waits for an answer. */
  readonly pending: boolean;
  /** When it was answered, in ms since the epoch. */
  readonly closedAt: number | undefined;
}

/**
 * The status of `call` at `now`. `turnEnded` is true when its turn has ended.
 *
 * While its approval waits, the call is paused: it shows no timer (design R4 item 5).
 * After an answer, the time counts from the answer, not from the start of the call.
 */
export function toolStatus(
  call: ToolCallView,
  turnEnded: boolean,
  now: number,
  approval?: ToolApproval,
): ToolStatus {
  if (!call.done && !turnEnded) {
    if (approval?.pending === true) {
      return { kind: "waiting" };
    }
    return { kind: "running", elapsed: elapsed(approval?.closedAt ?? call.startedAt, now) };
  }
  // NOTE: after an approval the time counts from the answer, as the loader did.
  const time = between(approval?.closedAt ?? call.startedAt, call.endedAt);
  const parts: string[] = [];
  if (call.exitCode !== undefined) {
    parts.push(String(call.exitCode));
  }
  if (!call.done) {
    // The turn ended first, such as at an interrupt: the call never answered.
    return { kind: "failed", detail: parts.length === 0 ? "stopped" : parts.join(" · ") };
  }
  if (call.refusal !== undefined) {
    return { kind: "failed", detail: "refused" };
  }
  if (call.tool === "read_file" && !call.isError && call.output !== undefined) {
    return { kind: "done", detail: lineCount(call.output) };
  }
  if (time !== undefined) {
    parts.push(time);
  }
  const detail = parts.length === 0 ? (call.isError ? "failed" : "done") : parts.join(" · ");
  return call.isError ? { kind: "failed", detail } : { kind: "done", detail };
}

/** `61 lines`, `1 line`. */
export function lineCount(text: string): string {
  const count = splitLines(text).length;
  return count === 1 ? "1 line" : `${String(count)} lines`;
}

/** A part of an output line: plain text, or an "error" word. */
export interface OutputSpan {
  /** Where the span begins in its line: its key in a view. */
  readonly start: number;
  readonly text: string;
  readonly error: boolean;
}

/** One line of output. */
export interface OutputLine {
  /** Stable while the output grows: the line's byte offset in the whole output. */
  readonly key: string;
  readonly text: string;
  readonly spans: readonly OutputSpan[];
}

/** The output lines that a block shows. */
export interface ToolOutput {
  readonly lines: readonly OutputLine[];
  /** How many lines the whole output has, as far as the app knows it. */
  readonly total: number;
  /** True when the expanded output is longer than MAX_OUTPUT_LINES. */
  readonly capped: boolean;
}

/** The output of `call` to show: the last 3 lines, or the whole output when `expanded`. */
export function toolOutput(call: ToolCallView, expanded: boolean): ToolOutput {
  if (call.tool === "read_file") {
    // A read shows its size in lines and no output (design 3.7).
    return { lines: [], total: 0, capped: false };
  }
  const notes: string[] = [];
  if (call.refusal !== undefined) {
    notes.push(call.refusal);
  }
  if (call.leftRunning) {
    notes.push("still runs in the shell");
  }
  // The whole output when efrd gave it; else the tail, the end of what came so far.
  const whole = call.done && call.output !== undefined && call.output.length > 0;
  const source = whole ? (call.output ?? "") : call.tail;
  const all = splitLines(source);
  const total = all.length + notes.length;
  const limit = expanded ? MAX_OUTPUT_LINES : TAIL_LINES;
  const from = Math.max(0, all.length - limit);
  // The byte offset of the source in the whole output: the tail is its end.
  const base = whole ? 0 : Math.max(0, call.bytes - utf8Length(source));
  const lines: OutputLine[] = [];
  let offset = utf8Length(all.slice(0, from).join("\n")) + (from > 0 ? 1 : 0);
  for (let i = from; i < all.length; i++) {
    const text = all[i] ?? "";
    lines.push({ key: `o${String(base + offset)}`, text, spans: errorSpans(text) });
    offset += utf8Length(text) + 1;
  }
  for (const note of notes) {
    lines.push({ key: `n${note}`, text: note, spans: [{ start: 0, text: note, error: false }] });
  }
  return { lines, total, capped: expanded && all.length > MAX_OUTPUT_LINES };
}

/** The label of the button under a block: `show all 14 lines`, or undefined when all show. */
export function moreLabel(output: ToolOutput, expanded: boolean): string | undefined {
  if (expanded) {
    return output.total > TAIL_LINES ? "collapse" : undefined;
  }
  return output.total > TAIL_LINES ? `show all ${String(output.total)} lines` : undefined;
}

/** `text` as plain spans and "error" words. */
export function errorSpans(text: string): OutputSpan[] {
  const spans: OutputSpan[] = [];
  const word = /\berror\b/gi;
  let at = 0;
  for (const match of text.matchAll(word)) {
    const start = match.index;
    if (start > at) {
      spans.push({ start: at, text: text.slice(at, start), error: false });
    }
    spans.push({ start, text: match[0], error: true });
    at = start + match[0].length;
  }
  if (at < text.length || spans.length === 0) {
    spans.push({ start: at, text: text.slice(at), error: false });
  }
  return spans;
}

/** The lines of `text`, without a final empty line; none for an empty text. */
function splitLines(text: string): string[] {
  if (text.length === 0) {
    return [];
  }
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines;
}

/** The header of a tool input that the model still writes: `writing patch` and its size. */
export interface WritingBlock {
  readonly verb: string;
  readonly size: string;
}

/** The writing block of `input` (design 3.7: only the size grows). */
export function writingBlock(input: ToolInputView): WritingBlock {
  const verb = input.tool === "apply_patch" ? "writing patch" : `writing ${input.tool}`;
  return { verb, size: size(input.bytes) };
}

/** The VoiceOver label of a tool block: what ran and how it ended. */
export function toolLabel(header: ToolHeader, status: ToolStatus): string {
  const what =
    header.verb === "$" ? `Command ${header.subject}` : `${header.verb} ${header.subject}`;
  switch (status.kind) {
    case "waiting":
      return `${what.trim()}, waiting for approval`;
    case "running":
      return `${what.trim()}, running${status.elapsed === undefined ? "" : `, ${status.elapsed}`}`;
    case "done":
      return `${what.trim()}, done, ${status.detail}`;
    default:
      return `${what.trim()}, failed, ${status.detail}`;
  }
}
