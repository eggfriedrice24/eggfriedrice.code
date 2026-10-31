// The line of a thought row (design 3.6, round 4 item 4 and the S-Messages sheet):
// "thinking" while the turn runs, "thought" after, then the title, then the time.
//
// - A bold `**Title**` in the reasoning is the title, in text_muted.
// - Without one (Claude writes none), the first words of the reasoning are the title,
//   in italic text_subtle, so it reads as a quote. They show once ~8 words have come, so
//   the line does not jitter, with the Markdown marks taken out. Expanded, they go: the
//   whole text is right below.
// - The time never truncates; the title does. Live it is the time since the turn
//   started; after, how long the first thought took.

import { between, elapsed } from "./durations";

/** How many words must come before the first words become the title. */
export const EXCERPT_MIN_WORDS = 8;

/** The most words of the first words; the line cuts them to its width anyway. */
const EXCERPT_MAX_WORDS = 24;

/** The parts of a thought row's line. */
export interface ThoughtLine {
  readonly word: "thinking" | "thought";
  /** A bold title of the reasoning. */
  readonly title: string | undefined;
  /** The first words of the reasoning, when it has no bold title. */
  readonly excerpt: string | undefined;
  /** `12s`, or undefined under 2 s or when the times are not known. */
  readonly time: string | undefined;
  /** The whole line in words, for VoiceOver. */
  readonly label: string;
}

export interface ThoughtLineInput {
  readonly title: string | undefined;
  /** The reasoning; "" when efrd did not keep it. */
  readonly text: string;
  readonly live: boolean;
  readonly expanded: boolean;
  readonly startedAt: number | undefined;
  /** When the first thought ended. */
  readonly endedAt: number | undefined;
  readonly now: number;
}

/** The line of a thought row. */
export function thoughtLine(input: ThoughtLineInput): ThoughtLine {
  const word = input.live ? "thinking" : "thought";
  const title = input.title === undefined || input.title === "" ? undefined : input.title;
  const excerpt =
    title === undefined && !input.expanded ? thoughtExcerpt(input.text, input.live) : undefined;
  const time = input.live
    ? elapsed(input.startedAt, input.now)
    : between(input.startedAt, input.endedAt);
  const label = [word, title ?? excerpt, time].filter((part) => part !== undefined).join(", ");
  return { word, title, excerpt, time, label };
}

/**
 * The first words of `text` without Markdown marks: undefined while a live text has
 * fewer than ~8 words.
 */
export function thoughtExcerpt(text: string, live: boolean): string | undefined {
  const words: string[] = [];
  for (const line of text.split("\n")) {
    const plain = line.replace(/^\s*(?:#{1,6}|>|[-+*]|\d+\.)\s+/u, "").replace(/[*`]|__/gu, "");
    words.push(...plain.split(/\s+/u).filter((part) => part !== ""));
    if (words.length >= EXCERPT_MAX_WORDS) {
      break;
    }
  }
  if (words.length === 0 || (live && words.length < EXCERPT_MIN_WORDS)) {
    return undefined;
  }
  return words.slice(0, EXCERPT_MAX_WORDS).join(" ");
}
